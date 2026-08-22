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
