use std::sync::{Arc, Mutex, OnceLock};

/// Snowflake ID 生成失败。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnowflakeError {
    #[error("工作机器 ID 必须在 0~{MAX_WORKER_ID} 之间，当前值: {worker_id}")]
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
/// ```
/// use ryframe_common::utils::snowflake::Snowflake;
///
/// let sf = Snowflake::new(1).expect("创建雪花算法实例失败");
/// let id = sf.try_next_id().expect("生成 Snowflake ID 失败");
/// assert!(id > 0);
///
/// let ts = Snowflake::extract_timestamp(id);
/// let wid = Snowflake::extract_worker_id(id);
/// assert_eq!(wid, 1);
/// assert!(ts > 1_769_660_800_000);
/// ```
pub struct Snowflake {
    /// 工作机器 ID（0~1023）。
    worker_id: i64,
    /// 时间戳和序列号必须作为一个整体更新，避免并发调用观察到不一致状态。
    state: Mutex<SnowflakeState>,
    time_source: Arc<dyn Fn() -> i64 + Send + Sync>,
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

/// 最大工作机器 ID。
const MAX_WORKER_ID: i64 = (1 << WORKER_ID_BITS) - 1;
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
        Self::with_time_source(worker_id, system_timestamp)
    }

    /// 使用自定义毫秒时间源创建生成器。
    ///
    /// 该构造函数适用于需要确定性时钟的测试，时间源返回 Unix 毫秒时间戳。
    pub fn with_time_source<F>(worker_id: i64, time_source: F) -> Result<Self, SnowflakeError>
    where
        F: Fn() -> i64 + Send + Sync + 'static,
    {
        validate_worker_id(worker_id)?;

        Ok(Self {
            worker_id,
            state: Mutex::new(SnowflakeState::default()),
            time_source: Arc::new(time_source),
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
        let observed_timestamp = (self.time_source)().max(EPOCH);

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
        (id >> WORKER_ID_LEFT_SHIFT) & MAX_WORKER_ID
    }
}

/// 读取并校验当前进程的 Snowflake worker ID。
///
/// 生产环境必须显式设置 `SNOWFLAKE_WORKER_ID`；开发和测试环境未设置时默认使用 1。
/// 如果变量已设置，则所有环境都会校验它是否为 0~1023 的整数。
pub fn worker_id_from_environment(environment: &str) -> Result<i64, String> {
    let production = matches!(
        environment.trim().to_ascii_lowercase().as_str(),
        "prod" | "production"
    );

    match std::env::var("SNOWFLAKE_WORKER_ID") {
        Ok(value) => {
            let worker_id = value.trim().parse::<i64>().map_err(|_| {
                format!("SNOWFLAKE_WORKER_ID 必须是 0~{MAX_WORKER_ID} 的整数，当前值: {value}")
            })?;
            validate_worker_id(worker_id).map_err(|error| error.to_string())?;
            Ok(worker_id)
        }
        Err(std::env::VarError::NotPresent) if production => {
            Err("生产环境必须显式设置 SNOWFLAKE_WORKER_ID，且每个应用实例必须使用不同值".into())
        }
        Err(std::env::VarError::NotPresent) => Ok(1),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("SNOWFLAKE_WORKER_ID 必须是有效的 UTF-8 整数".into())
        }
    }
}

fn validate_worker_id(worker_id: i64) -> Result<(), SnowflakeError> {
    if !(0..=MAX_WORKER_ID).contains(&worker_id) {
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

/// 全局默认雪花算法实例。
///
/// `SNOWFLAKE_WORKER_ID` 只会在实例首次使用时读取一次。生产环境必须显式配置；
/// 开发和测试环境未配置时使用 worker ID 1。
pub fn default_snowflake() -> Result<&'static Snowflake, SnowflakeError> {
    static INSTANCE: OnceLock<Result<Snowflake, SnowflakeError>> = OnceLock::new();
    match INSTANCE.get_or_init(|| {
        let environment = std::env::var("APP_ENV").unwrap_or_else(|_| "dev".into());
        let worker_id =
            worker_id_from_environment(&environment).map_err(SnowflakeError::Configuration)?;
        Snowflake::new(worker_id)
    }) {
        Ok(snowflake) => Ok(snowflake),
        Err(error) => Err(error.clone()),
    }
}

/// 便捷函数：尝试生成一个进程内唯一且单调递增的 ID。
pub fn try_next_snowflake_id() -> Result<i64, SnowflakeError> {
    default_snowflake()?.try_next_id()
}
