//! 弹性容错工具：重试、熔断器
//!
//! 使用示例：
//! ```ignore
//! use ryframe_core::resilience::{RetryConfig, retry_with_backoff};
//!
//! let result = retry_with_backoff(
//!     || async { db.find_by_id(1).await },
//!     &RetryConfig::default(),
//! ).await;
//! ```

use std::{
    future::Future,
    sync::{
        RwLock,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use tracing::{info, warn};

/// 重试配置
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// 最大重试次数
    pub max_retries: u32,
    /// 初始退避时间（毫秒）
    pub initial_backoff_ms: u64,
    /// 最大退避时间（毫秒）
    pub max_backoff_ms: u64,
    /// 退避乘数
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5_000,
            backoff_multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    /// 快速重试（用于 Redis 等快速操作）
    pub fn fast() -> Self {
        Self {
            max_retries: 2,
            initial_backoff_ms: 10,
            max_backoff_ms: 100,
            backoff_multiplier: 2.0,
        }
    }

    /// 持久重试（用于数据库连接恢复等场景）
    pub fn persistent() -> Self {
        Self {
            max_retries: 5,
            initial_backoff_ms: 500,
            max_backoff_ms: 10_000,
            backoff_multiplier: 2.0,
        }
    }
}

/// 按退避策略重试异步操作
///
/// 当 `f` 返回 `Err` 时，等待退避时间后重试，最多 `config.max_retries` 次。
/// 首次调用不计入重试次数。
pub async fn retry_with_backoff<F, Fut, T, E>(mut f: F, config: &RetryConfig) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut attempt = 0;
    let mut backoff_ms = config.initial_backoff_ms;

    loop {
        match f().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                attempt += 1;
                if attempt > config.max_retries {
                    warn!(
                        "重试耗尽 (max_retries={}, last_error={})",
                        config.max_retries, e
                    );
                    return Err(e);
                }

                warn!(
                    "操作失败，{}ms 后重试 (attempt={}/{}, error={})",
                    backoff_ms, attempt, config.max_retries, e
                );

                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;

                // 指数退避
                backoff_ms = ((backoff_ms as f64) * config.backoff_multiplier)
                    .min(config.max_backoff_ms as f64) as u64;
            }
        }
    }
}

/// 熔断器状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// 闭合（正常工作）
    Closed,
    /// 断开（熔断中）
    Open,
    /// 半开（尝试恢复）
    HalfOpen,
}

/// 简单熔断器
///
/// 基于失败计数和时间窗口的熔断器：
/// - 连续失败达到阈值 → 进入 Open 状态
/// - Open 状态持续 `timeout` 后 → 进入 HalfOpen 状态
/// - HalfOpen 状态下成功 `half_open_max` 次 → 恢复到 Closed
/// - HalfOpen 状态下任何失败 → 回到 Open
pub struct CircuitBreaker {
    /// 熔断阈值（连续失败次数达到阈值时熔断）
    failure_threshold: u32,
    /// 熔断恢复超时
    timeout: Duration,
    /// HalfOpen 状态下需要连续成功多少次才恢复
    half_open_max: u32,
    /// 当前失败计数
    failure_count: AtomicU32,
    /// HalfOpen 状态下的成功计数
    half_open_success: AtomicU32,
    /// 状态变更时间
    state_changed_at: RwLock<Instant>,
    /// 当前状态
    state: RwLock<CircuitState>,
}

impl CircuitBreaker {
    /// 创建新熔断器
    pub fn new(failure_threshold: u32, timeout_secs: u64, half_open_max: u32) -> Self {
        Self {
            failure_threshold,
            timeout: Duration::from_secs(timeout_secs),
            half_open_max,
            failure_count: AtomicU32::new(0),
            half_open_success: AtomicU32::new(0),
            state_changed_at: RwLock::new(Instant::now()),
            state: RwLock::new(CircuitState::Closed),
        }
    }

    /// 默认配置：连续 5 次失败熔断，30 秒后恢复尝试，3 次成功关闭
    pub fn default_config() -> Self {
        Self::new(5, 30, 3)
    }

    /// 检查是否可以尝试执行操作
    ///
    /// 返回 `true` 表示允许执行，`false` 表示熔断中
    pub fn allow_request(&self) -> bool {
        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                let elapsed = self.state_changed_at.read().unwrap().elapsed();
                if elapsed >= self.timeout {
                    // 超时到期，切换到 HalfOpen
                    let mut s = self.state.write().unwrap();
                    let mut t = self.state_changed_at.write().unwrap();
                    *s = CircuitState::HalfOpen;
                    *t = Instant::now();
                    self.half_open_success.store(0, Ordering::SeqCst);
                    info!("熔断器进入 HalfOpen 状态，尝试恢复");
                    true
                } else {
                    false
                }
            }
        }
    }

    /// 记录操作成功
    pub fn record_success(&self) {
        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed => {
                self.failure_count.store(0, Ordering::SeqCst);
            }
            CircuitState::HalfOpen => {
                let success = self.half_open_success.fetch_add(1, Ordering::SeqCst) + 1;
                if success >= self.half_open_max {
                    let mut s = self.state.write().unwrap();
                    *s = CircuitState::Closed;
                    self.failure_count.store(0, Ordering::SeqCst);
                    self.half_open_success.store(0, Ordering::SeqCst);
                    let mut t = self.state_changed_at.write().unwrap();
                    *t = Instant::now();
                    info!("熔断器恢复正常（Closed）");
                }
            }
            CircuitState::Open => {
                // 不应出现，但做个防御
                self.failure_count.store(0, Ordering::SeqCst);
            }
        }
    }

    /// 记录操作失败
    pub fn record_failure(&self) {
        let state = *self.state.read().unwrap();
        match state {
            CircuitState::Closed => {
                let count = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count >= self.failure_threshold {
                    let mut s = self.state.write().unwrap();
                    *s = CircuitState::Open;
                    let mut t = self.state_changed_at.write().unwrap();
                    *t = Instant::now();
                    warn!(
                        "熔断器触发（Open）: 连续失败 {} 次，将在 {}s 后尝试恢复",
                        count,
                        self.timeout.as_secs()
                    );
                }
            }
            CircuitState::HalfOpen => {
                // HalfOpen 状态下失败，立即回到 Open
                let mut s = self.state.write().unwrap();
                *s = CircuitState::Open;
                self.half_open_success.store(0, Ordering::SeqCst);
                let mut t = self.state_changed_at.write().unwrap();
                *t = Instant::now();
                warn!("熔断器恢复失败，重新进入 Open 状态");
            }
            CircuitState::Open => {
                // 已熔断，重置失败计数
                self.failure_count.store(0, Ordering::SeqCst);
            }
        }
    }

    /// 获取当前状态
    pub fn current_state(&self) -> CircuitState {
        *self.state.read().unwrap()
    }
}
