use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use time::{OffsetDateTime, PrimitiveDateTime};

/// 一次实体写入共用的自动填充上下文。
pub struct FillContext {
    pub now: DateTime<Utc>,
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

impl FillContext {
    pub fn new() -> Self {
        Self {
            now: Utc::now(),
            user_id: None,
            username: None,
        }
    }

    pub(crate) fn sql_utc_datetime(&self) -> AppResult<PrimitiveDateTime> {
        let utc = OffsetDateTime::from_unix_timestamp(self.now.timestamp())
            .and_then(|instant| instant.replace_nanosecond(self.now.timestamp_subsec_nanos()))
            .map_err(|_| AppError::Database("自动填充时间无法精确表示为 UTC".into()))?;
        Ok(PrimitiveDateTime::new(utc.date(), utc.time()))
    }
}

impl Default for FillContext {
    fn default() -> Self {
        Self::new()
    }
}

/// 控制库实体在写入前执行的自动填充行为。
pub trait AutoFill {
    fn fill_on_insert(&mut self, context: &FillContext) -> AppResult<()>;
    fn fill_on_update(&mut self, context: &FillContext) -> AppResult<()>;
}
