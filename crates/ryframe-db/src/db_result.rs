use std::{any::Any, fmt::Display};

use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DbErr, RuntimeErr, sqlx};

/// 将结构化外键错误转换为业务冲突，其余错误保持数据库错误语义。
pub trait DbResultExt<T> {
    /// 外键错误返回安全的冲突建议，其余错误保留底层文本。
    fn db(self) -> AppResult<T>;

    /// 在非外键错误的底层文本前附加稳定的操作上下文。
    fn db_context(self, context: &'static str) -> AppResult<T>;

    /// 外键错误与显式唯一键冲突返回稳定业务冲突，其余错误保留文本。
    fn db_conflicts(self, conflicts: &[(&str, &str)], fallback: &str) -> AppResult<T>;
}

impl<T, E> DbResultExt<T> for Result<T, E>
where
    E: Display + 'static,
{
    fn db(self) -> AppResult<T> {
        self.map_err(|error| {
            foreign_key_conflict(&error).unwrap_or_else(|| AppError::Database(error.to_string()))
        })
    }

    fn db_context(self, context: &'static str) -> AppResult<T> {
        self.map_err(|error| {
            foreign_key_conflict(&error)
                .unwrap_or_else(|| AppError::Database(format!("{context}: {error}")))
        })
    }

    fn db_conflicts(self, conflicts: &[(&str, &str)], fallback: &str) -> AppResult<T> {
        self.map_err(|error| {
            if let Some(conflict) = foreign_key_conflict(&error) {
                return conflict;
            }
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

fn foreign_key_conflict(error: &dyn Any) -> Option<AppError> {
    let error = error.downcast_ref::<DbErr>()?;
    let (DbErr::Exec(RuntimeErr::SqlxError(error)) | DbErr::Query(RuntimeErr::SqlxError(error))) =
        error
    else {
        return None;
    };
    let sqlx::Error::Database(error) = error.as_ref() else {
        return None;
    };
    // 由驱动识别 MySQL 1451/1452，不能从可能包含 SQL 或字段值的错误文本推断。
    error.is_foreign_key_violation().then(|| {
        AppError::Conflict(
            "数据关联约束冲突，请先处理引用此记录的数据，并确认所引用的记录存在后重试".into(),
        )
    })
}
