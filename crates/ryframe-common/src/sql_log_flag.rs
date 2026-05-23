use std::sync::atomic::{AtomicBool, Ordering};

/// 全局 SQL 完整日志开关（仅 full 模式为 true）
static SQL_FULL_LOG: AtomicBool = AtomicBool::new(false);

/// 开启完整 SQL 结果日志（full 模式调用）
pub fn enable_sql_full_log() {
    SQL_FULL_LOG.store(true, Ordering::Release);
}

/// 检查当前是否为 full 模式
pub fn is_sql_full_log() -> bool {
    SQL_FULL_LOG.load(Ordering::Acquire)
}
