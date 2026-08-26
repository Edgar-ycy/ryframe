use std::fmt::Display;

use ryframe_kernel::{AppError, AppResult};

/// 将底层数据库错误统一转换为应用数据库错误。
pub trait DbResultExt<T> {
    /// 保留底层错误文本并映射为数据库错误。
    fn db(self) -> AppResult<T>;

    /// 在底层错误文本前附加稳定的操作上下文。
    fn db_context(self, context: &'static str) -> AppResult<T>;

    /// 将唯一键冲突映射为稳定业务冲突，其余错误保持数据库错误文本。
    fn db_conflicts(self, conflicts: &[(&str, &str)], fallback: &str) -> AppResult<T>;
}

impl<T, E> DbResultExt<T> for Result<T, E>
where
    E: Display,
{
    fn db(self) -> AppResult<T> {
        self.map_err(|error| AppError::Database(error.to_string()))
    }

    fn db_context(self, context: &'static str) -> AppResult<T> {
        self.map_err(|error| AppError::Database(format!("{context}: {error}")))
    }

    fn db_conflicts(self, conflicts: &[(&str, &str)], fallback: &str) -> AppResult<T> {
        self.map_err(|error| {
            let message = error.to_string();
            let normalized = message.to_ascii_lowercase();
            if normalized.contains("1062") || normalized.contains("duplicate entry") {
                let conflict = conflicts
                    .iter()
                    .find(|(index, _)| normalized.contains(&index.to_ascii_lowercase()))
                    .map_or(fallback, |(_, message)| *message);
                AppError::Conflict(conflict.into())
            } else {
                AppError::Database(message)
            }
        })
    }
}
