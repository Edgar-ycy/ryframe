use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

/// 雪花算法 ID 生成器
///
/// 格式：1位符号位(恒0) + 41位时间戳 + 10位工作机器ID + 12位序列号
/// - 时间戳：从自定义起始时间（2026-01-01 00:00:00 UTC）起的毫秒数
/// - 工作机器ID：10位，最多支持 1024 个节点（0~1023）
/// - 序列号：12位，同一毫秒内最多生成 4096 个 ID（0~4095）
///
/// # 使用方式
///
/// ```ignore
/// use ryframe_common::utils::snowflake::Snowflake;
///
/// let sf = Snowflake::new(1).expect("创建雪花算法实例失败");
/// let id = sf.next_id();
/// ```
pub struct Snowflake {
    /// 工作机器ID（0~1023）
    worker_id: i64,
    /// 序列号
    sequence: AtomicI64,
    /// 上次生成 ID 的时间戳（毫秒）
    last_timestamp: Mutex<i64>,
}

/// 自定义起始时间：2026-01-01 00:00:00 UTC（毫秒时间戳）
const EPOCH: i64 = 1_769_660_800_000;

/// 工作机器ID占用的位数
const WORKER_ID_BITS: i64 = 10;
/// 序列号占用的位数
const SEQUENCE_BITS: i64 = 12;

/// 最大工作机器ID
const MAX_WORKER_ID: i64 = -1i64 ^ (-1i64 << WORKER_ID_BITS);
/// 最大序列号
const MAX_SEQUENCE: i64 = -1i64 ^ (-1i64 << SEQUENCE_BITS);

/// 时间戳左移位数
const TIMESTAMP_LEFT_SHIFT: i64 = WORKER_ID_BITS + SEQUENCE_BITS;
/// 工作机器ID左移位数
const WORKER_ID_LEFT_SHIFT: i64 = SEQUENCE_BITS;

impl Snowflake {
    /// 创建一个新的雪花算法实例
    ///
    /// # 参数
    /// * `worker_id` - 工作机器ID，范围 0~1023
    ///
    /// # 错误
    /// 如果 `worker_id` 超出范围则返回错误
    pub fn new(worker_id: i64) -> Result<Self, String> {
        if !(0..=MAX_WORKER_ID).contains(&worker_id) {
            return Err(format!(
                "工作机器ID必须在 0~{} 之间，当前值: {}",
                MAX_WORKER_ID, worker_id
            ));
        }

        Ok(Self {
            worker_id,
            sequence: AtomicI64::new(0),
            last_timestamp: Mutex::new(0),
        })
    }

    /// 生成下一个唯一ID
    ///
    /// 线程安全，可以在多线程环境下并发调用。
    pub fn next_id(&self) -> i64 {
        let mut last_ts = self.last_timestamp.lock().unwrap();
        let mut current_ts = self.current_timestamp();

        // 检查时钟回拨
        if current_ts < *last_ts {
            // 如果时钟回拨在可接受范围内（3ms），等待直到追上
            let mut waited = 0;
            while current_ts < *last_ts && waited < 100 {
                std::thread::sleep(std::time::Duration::from_millis(1));
                current_ts = self.current_timestamp();
                waited += 1;
            }

            if current_ts < *last_ts {
                // 时钟回拨超过可接受范围，使用上次时间戳继续（容忍小幅度回拨）
                current_ts = *last_ts;
            }
        }

        if current_ts == *last_ts {
            // 同一毫秒内，序列号递增
            let seq = self.sequence.fetch_add(1, Ordering::SeqCst) & MAX_SEQUENCE;
            if seq == 0 {
                // 序列号用完，等待到下一毫秒
                while current_ts <= *last_ts {
                    current_ts = self.current_timestamp();
                }
                self.sequence.store(0, Ordering::SeqCst);
            }
        } else {
            // 不同毫秒，序列号重置为0
            self.sequence.store(0, Ordering::SeqCst);
        }

        *last_ts = current_ts;

        // 组合生成 ID
        ((current_ts - EPOCH) << TIMESTAMP_LEFT_SHIFT)
            | (self.worker_id << WORKER_ID_LEFT_SHIFT)
            | (self.sequence.load(Ordering::SeqCst))
    }

    /// 从 ID 中提取时间戳
    pub fn extract_timestamp(id: i64) -> i64 {
        (id >> TIMESTAMP_LEFT_SHIFT) + EPOCH
    }

    /// 从 ID 中提取工作机器ID
    pub fn extract_worker_id(id: i64) -> i64 {
        (id >> WORKER_ID_LEFT_SHIFT) & MAX_WORKER_ID
    }

    /// 获取当前毫秒时间戳
    fn current_timestamp(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }
}

/// 全局默认雪花算法实例（工作机器ID为1）
///
/// 在分布式环境下应根据配置为每个节点分配不同的 worker_id。
/// 可通过环境变量 `SNOWFLAKE_WORKER_ID` 或配置文件来设置。
pub fn default_snowflake() -> &'static Snowflake {
    use std::sync::OnceLock;
    static INSTANCE: OnceLock<Snowflake> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let worker_id = std::env::var("SNOWFLAKE_WORKER_ID")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(1);
        Snowflake::new(worker_id).expect("默认雪花算法初始化失败")
    })
}

/// 便捷函数：生成一个全局唯一 ID
pub fn next_snowflake_id() -> i64 {
    default_snowflake().next_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_new_with_valid_worker_id() {
        assert!(Snowflake::new(0).is_ok());
        assert!(Snowflake::new(512).is_ok());
        assert!(Snowflake::new(1023).is_ok());
    }

    #[test]
    fn test_new_with_invalid_worker_id() {
        assert!(Snowflake::new(-1).is_err());
        assert!(Snowflake::new(1024).is_err());
    }

    #[test]
    fn test_id_uniqueness() {
        let sf = Snowflake::new(1).unwrap();
        let mut ids = HashSet::new();

        for _ in 0..10000 {
            let id = sf.next_id();
            assert!(ids.insert(id), "重复ID: {}", id);
        }
    }

    #[test]
    fn test_id_is_positive() {
        let sf = Snowflake::new(1).unwrap();
        for _ in 0..1000 {
            assert!(sf.next_id() > 0);
        }
    }

    #[test]
    fn test_extract_timestamp() {
        let sf = Snowflake::new(1).unwrap();
        let id = sf.next_id();
        let ts = Snowflake::extract_timestamp(id);
        // 时间戳应该在合理范围内
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(ts <= now + 1000);
        assert!(ts >= now - 10000);
    }

    #[test]
    fn test_extract_worker_id() {
        let sf = Snowflake::new(42).unwrap();
        let id = sf.next_id();
        assert_eq!(Snowflake::extract_worker_id(id), 42);
    }

    #[test]
    fn test_different_workers_produce_different_ids() {
        let sf1 = Snowflake::new(1).unwrap();
        let sf2 = Snowflake::new(2).unwrap();

        for _ in 0..100 {
            assert_ne!(sf1.next_id(), sf2.next_id());
        }
    }

    #[test]
    fn test_default_snowflake() {
        let id1 = next_snowflake_id();
        let id2 = next_snowflake_id();
        assert!(id1 > 0);
        assert!(id2 > 0);
        assert_ne!(id1, id2);
    }
}
