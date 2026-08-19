use std::sync::{Mutex, OnceLock};

use ryframe_kernel::{AppError, MAX_SNOWFLAKE_WORKER_ID};

/// Snowflake ID 生成失败。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnowflakeError {
    #[error("工作机器 ID 必须在 0~{MAX_SNOWFLAKE_WORKER_ID} 之间，当前值: {worker_id}")]
    InvalidWorkerId { worker_id: i64 },
    #[error("系统时钟发生回拨（上次时间戳: {last_timestamp}，当前时间戳: {observed_timestamp}）")]
    ClockMovedBackwards {
        last_timestamp: i64,
        observed_timestamp: i64,
    },
    #[error("时间戳 {timestamp} 超出 Snowflake 41 位时间范围")]
    TimestampOutOfRange { timestamp: i64 },
    #[error("时间戳 {timestamp} 的 4096 个 Snowflake 序列号已耗尽")]
    SequenceExhausted { timestamp: i64 },
    #[error("Snowflake 配置无效: {0}")]
    Configuration(String),
}

/// 雪花算法 ID 生成器。
///
/// 格式：1 位符号位（恒 0）+ 41 位时间戳 + 10 位工作机器 ID + 12 位序列号。
///
/// - 时间戳：从自定义起始时间（2026-01-01 00:00:00 UTC）起的毫秒数
/// - 工作机器 ID：10 位，最多支持 1024 个节点（0~1023）
/// - 序列号：12 位，同一毫秒内最多生成 4096 个 ID（0~4095）
///
/// 生成器不会等待物理时钟，也不会生成逻辑上的未来时间戳。系统时钟回拨或同一毫秒
/// 的序列号耗尽时，[`Snowflake::try_next_id`] 会立即返回可重试错误，调用方可以将其
/// 转换为受控的服务不可用响应，而不会阻塞 Tokio 工作线程或触发 panic。
///
/// 唯一性和单调性保证覆盖当前进程生命周期。生成器不持久化时间戳高水位；进程重启后
/// 如果复用同一个 worker ID 且物理时钟回拨到已使用过的毫秒（或在同一毫秒内重启），
/// 仍可能与重启前的 ID 冲突。生产环境必须保证 worker ID 独占，并在复用前确保物理时钟
/// 已超过该 worker 最后生成 ID 的时间戳；需要跨重启严格保证时应使用外部持久化协调。
///
/// # 使用方式
///
/// ```text
/// use ryframe_adapters::snowflake::Snowflake;
///
/// let sf = Snowflake::new(1).expect("创建雪花算法实例失败");
/// let id = sf.try_next_id().expect("生成 Snowflake ID 失败");
///
/// let ts = Snowflake::extract_timestamp(id);
/// let wid = Snowflake::extract_worker_id(id);
/// ```
pub struct Snowflake {
    /// 工作机器 ID（0~1023）。
    worker_id: i64,
    /// 时间戳和序列号必须作为一个整体更新，避免并发调用观察到不一致状态。
    state: Mutex<SnowflakeState>,
}

#[derive(Debug, Default)]
struct SnowflakeState {
    last_timestamp: i64,
    sequence: i64,
}

/// 自定义起始时间：2026-01-01 00:00:00 UTC（毫秒时间戳）。
const EPOCH: i64 = 1_769_660_800_000;

/// 工作机器 ID 占用的位数。
const WORKER_ID_BITS: i64 = 10;
/// 序列号占用的位数。
const SEQUENCE_BITS: i64 = 12;

/// 最大序列号。
const MAX_SEQUENCE: i64 = (1 << SEQUENCE_BITS) - 1;
/// 41 位时间戳能够表示的最大 Unix 毫秒时间戳。
const MAX_TIMESTAMP: i64 = EPOCH + ((1 << 41) - 1);

/// 时间戳左移位数。
const TIMESTAMP_LEFT_SHIFT: i64 = WORKER_ID_BITS + SEQUENCE_BITS;
/// 工作机器 ID 左移位数。
const WORKER_ID_LEFT_SHIFT: i64 = SEQUENCE_BITS;

impl Snowflake {
    /// 创建一个新的雪花算法实例。
    ///
    /// # 参数
    ///
    /// * `worker_id` - 工作机器 ID，范围 0~1023
    ///
    /// # 错误
    ///
    /// 如果 `worker_id` 超出范围则返回错误。
    pub fn new(worker_id: i64) -> Result<Self, SnowflakeError> {
        validate_worker_id(worker_id)?;

        Ok(Self {
            worker_id,
            state: Mutex::new(SnowflakeState::default()),
        })
    }

    /// 尝试生成下一个唯一 ID。
    ///
    /// 线程安全，可以在多线程环境下并发调用。成功返回的 ID 按照状态锁的获取顺序严格
    /// 递增。时钟回拨和单毫秒序列耗尽会立即返回错误，状态保持不变；调用方可稍后重试。
    pub fn try_next_id(&self) -> Result<i64, SnowflakeError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 在锁内读取时间，避免并发线程先读时间、后以相反顺序取得锁而被误判为回拨。
        // 系统时间早于自定义纪元时，以纪元为下限，避免生成负数 ID。
        let observed_timestamp = system_timestamp().max(EPOCH);

        if observed_timestamp < state.last_timestamp {
            return Err(SnowflakeError::ClockMovedBackwards {
                last_timestamp: state.last_timestamp,
                observed_timestamp,
            });
        }
        if observed_timestamp > MAX_TIMESTAMP {
            return Err(SnowflakeError::TimestampOutOfRange {
                timestamp: observed_timestamp,
            });
        }

        if observed_timestamp > state.last_timestamp {
            state.last_timestamp = observed_timestamp;
            state.sequence = 0;
        } else if state.sequence < MAX_SEQUENCE {
            // 同一毫秒内继续递增序列号。
            state.sequence += 1;
        } else {
            return Err(SnowflakeError::SequenceExhausted {
                timestamp: state.last_timestamp,
            });
        }

        Ok(((state.last_timestamp - EPOCH) << TIMESTAMP_LEFT_SHIFT)
            | (self.worker_id << WORKER_ID_LEFT_SHIFT)
            | state.sequence)
    }

    /// 从 ID 中提取时间戳。
    pub fn extract_timestamp(id: i64) -> i64 {
        (id >> TIMESTAMP_LEFT_SHIFT) + EPOCH
    }

    /// 从 ID 中提取工作机器 ID。
    pub fn extract_worker_id(id: i64) -> i64 {
        (id >> WORKER_ID_LEFT_SHIFT) & MAX_SNOWFLAKE_WORKER_ID
    }
}

/// 校验 Snowflake worker ID 是否处于可编码范围。
pub fn validate_worker_id(worker_id: i64) -> Result<(), SnowflakeError> {
    if !(0..=MAX_SNOWFLAKE_WORKER_ID).contains(&worker_id) {
        return Err(SnowflakeError::InvalidWorkerId { worker_id });
    }
    Ok(())
}

fn system_timestamp() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

static DEFAULT_SNOWFLAKE: OnceLock<Snowflake> = OnceLock::new();

/// 在进程启动边界初始化全局 Snowflake 实例。
///
/// 使用相同 worker ID 重复初始化是幂等的；尝试切换为其他 worker ID 会返回配置错误。
pub fn initialize(worker_id: i64) -> Result<(), SnowflakeError> {
    validate_worker_id(worker_id)?;
    if let Some(existing) = DEFAULT_SNOWFLAKE.get() {
        return if existing.worker_id == worker_id {
            Ok(())
        } else {
            Err(SnowflakeError::Configuration(format!(
                "Snowflake 已使用 worker ID {} 初始化，不能切换为 {worker_id}",
                existing.worker_id
            )))
        };
    }

    let snowflake = Snowflake::new(worker_id)?;
    match DEFAULT_SNOWFLAKE.set(snowflake) {
        Ok(()) => Ok(()),
        Err(_) => {
            let existing = DEFAULT_SNOWFLAKE
                .get()
                .expect("并发初始化完成后实例必须存在");
            if existing.worker_id == worker_id {
                Ok(())
            } else {
                Err(SnowflakeError::Configuration(format!(
                    "Snowflake 已使用 worker ID {} 初始化，不能切换为 {worker_id}",
                    existing.worker_id
                )))
            }
        }
    }
}

/// 返回已在进程启动边界初始化的全局 Snowflake 实例。
pub fn default_snowflake() -> Result<&'static Snowflake, SnowflakeError> {
    DEFAULT_SNOWFLAKE
        .get()
        .ok_or_else(|| SnowflakeError::Configuration("Snowflake 尚未在进程启动边界初始化".into()))
}

/// 便捷函数：尝试生成一个进程内唯一且单调递增的 ID。
pub fn try_next_snowflake_id() -> Result<i64, SnowflakeError> {
    default_snowflake()?.try_next_id()
}

impl From<SnowflakeError> for AppError {
    fn from(error: SnowflakeError) -> Self {
        tracing::error!(%error, "Snowflake ID 生成失败");
        Self::ServiceUnavailable("ID 生成服务暂时不可用，请稍后重试".into())
    }
}
