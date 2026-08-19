//! 弹性容错工具：重试、熔断器
//!
//! 使用示例：
//! ```text
//! # use ryframe_adapters::resilience::{RetryConfig, retry_with_backoff};
//! # use ryframe_adapters::resilience::CircuitBreaker;
//! # #[tokio::main]
//! # async fn main() {
//! // 重试机制
//! let result = retry_with_backoff(
//!     || async { Ok::<_, &str>("success") },
//!     &RetryConfig::default(),
//! ).await;
//!
//! // 熔断器
//! let breaker = CircuitBreaker::default_config();
//! breaker.record_success();
//! # }
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
    /// `HalfOpen` 状态下需要连续成功多少次才恢复
    half_open_max: u32,
    /// 当前失败计数
    failure_count: AtomicU32,
    /// 当前状态以及 HalfOpen 探测统计
    ///
    /// 这些值必须在同一把锁下更新，否则从 Open 切换到 HalfOpen 时，
    /// 并发请求可能重复重置计数并突破探测上限。
    status: RwLock<CircuitStatus>,
}

#[derive(Debug)]
struct CircuitStatus {
    state: CircuitState,
    changed_at: Instant,
    half_open_success: u32,
    half_open_in_flight: u32,
}

impl CircuitBreaker {
    /// 创建新熔断器
    pub fn new(failure_threshold: u32, timeout_secs: u64, half_open_max: u32) -> Self {
        assert!(half_open_max > 0, "half_open_max must be greater than zero");

        Self {
            failure_threshold,
            timeout: Duration::from_secs(timeout_secs),
            half_open_max,
            failure_count: AtomicU32::new(0),
            status: RwLock::new(CircuitStatus {
                state: CircuitState::Closed,
                changed_at: Instant::now(),
                half_open_success: 0,
                half_open_in_flight: 0,
            }),
        }
    }

    /// 默认配置：连续 5 次失败熔断，30 秒后恢复尝试，3 次成功关闭
    pub fn default_config() -> Self {
        Self::new(5, 30, 3)
    }

    /// 获取状态写锁；锁中毒时按保守策略恢复为 Open 状态。
    fn status_guard(&self) -> std::sync::RwLockWriteGuard<'_, CircuitStatus> {
        match self.status.write() {
            Ok(status) => status,
            Err(poisoned) => {
                warn!("熔断器状态锁已中毒，按 Open 状态恢复");
                let mut status = poisoned.into_inner();
                status.state = CircuitState::Open;
                status.changed_at = Instant::now();
                status.half_open_success = 0;
                status.half_open_in_flight = 0;
                self.failure_count
                    .store(self.failure_threshold, Ordering::SeqCst);
                self.status.clear_poison();
                status
            }
        }
    }

    /// 检查是否可以尝试执行操作
    ///
    /// 返回 `true` 表示允许执行，`false` 表示熔断中
    pub fn allow_request(&self) -> bool {
        let mut status = self.status_guard();
        match status.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => {
                // 一个 `HalfOpen` 世代最多只能发出 half_open_max 次探测。将已完成的成功次数
                // 计入已用配额，可防止先前已准入的探测仍在执行时熔断器提前关闭。
                if status.half_open_success + status.half_open_in_flight >= self.half_open_max {
                    false
                } else {
                    status.half_open_in_flight += 1;
                    true
                }
            }
            CircuitState::Open => {
                let elapsed = status.changed_at.elapsed();
                if elapsed >= self.timeout {
                    // 超时到期，切换到 HalfOpen
                    status.state = CircuitState::HalfOpen;
                    status.changed_at = Instant::now();
                    status.half_open_success = 0;
                    status.half_open_in_flight = 1;
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
        let mut status = self.status_guard();
        match status.state {
            CircuitState::Closed => {
                self.failure_count.store(0, Ordering::SeqCst);
            }
            CircuitState::HalfOpen => {
                // 只接受由 allow_request 发出的探测结果，避免未获许可的
                // 调用推进 HalfOpen 成功计数。
                if status.half_open_in_flight == 0 {
                    return;
                }

                status.half_open_in_flight -= 1;
                status.half_open_success += 1;
                if status.half_open_success >= self.half_open_max {
                    status.state = CircuitState::Closed;
                    self.failure_count.store(0, Ordering::SeqCst);
                    status.half_open_success = 0;
                    status.half_open_in_flight = 0;
                    status.changed_at = Instant::now();
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
        let mut status = self.status_guard();
        match status.state {
            CircuitState::Closed => {
                let count = self.failure_count.fetch_add(1, Ordering::SeqCst) + 1;
                if count >= self.failure_threshold {
                    status.state = CircuitState::Open;
                    status.changed_at = Instant::now();
                    warn!(
                        "熔断器触发（Open）: 连续失败 {} 次，将在 {}s 后尝试恢复",
                        count,
                        self.timeout.as_secs()
                    );
                }
            }
            CircuitState::HalfOpen => {
                // `HalfOpen` 状态下失败，立即回到 `Open`
                status.state = CircuitState::Open;
                status.half_open_success = 0;
                status.half_open_in_flight = 0;
                status.changed_at = Instant::now();
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
        self.status_guard().state
    }
}
