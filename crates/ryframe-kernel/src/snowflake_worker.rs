/// Snowflake 10 位 worker 字段允许的最大值。
pub const MAX_SNOWFLAKE_WORKER_ID: i64 = 1023;

/// 已校验的 Snowflake worker ID。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnowflakeWorkerId(i64);

impl SnowflakeWorkerId {
    pub const fn new(value: i64) -> Option<Self> {
        if value >= 0 && value <= MAX_SNOWFLAKE_WORKER_ID {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SNOWFLAKE_WORKER_ID, SnowflakeWorkerId};

    #[test]
    fn worker_id_is_bounded_by_encoded_bits() {
        assert_eq!(
            SnowflakeWorkerId::new(0).map(SnowflakeWorkerId::get),
            Some(0)
        );
        assert!(SnowflakeWorkerId::new(MAX_SNOWFLAKE_WORKER_ID).is_some());
        assert!(SnowflakeWorkerId::new(-1).is_none());
        assert!(SnowflakeWorkerId::new(MAX_SNOWFLAKE_WORKER_ID + 1).is_none());
    }
}
