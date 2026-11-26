use ryframe_kernel::{AppError, AppResult};
use time::{OffsetDateTime, PrimitiveDateTime, UtcOffset};

/// 一次实体写入共用的自动填充上下文。
pub struct FillContext {
    pub now: OffsetDateTime,
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

impl FillContext {
    pub fn new() -> Self {
        Self {
            now: OffsetDateTime::now_utc(),
            user_id: None,
            username: None,
        }
    }

    pub(crate) fn sql_utc_datetime(&self) -> AppResult<PrimitiveDateTime> {
        let utc = self
            .now
            .checked_to_offset(UtcOffset::UTC)
            .ok_or_else(|| AppError::Database("自动填充时间无法精确表示为 UTC".into()))?;
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
