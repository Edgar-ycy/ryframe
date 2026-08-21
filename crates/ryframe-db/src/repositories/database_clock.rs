use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, Statement};

/// 读取主数据库的 UTC 时钟，统一租约、过期和任务状态的时间基准。
pub async fn utc_now<C>(database: &C) -> AppResult<DateTime<Utc>>
where
    C: ConnectionTrait + ?Sized,
{
    let row = database
        .query_one_raw(Statement::from_string(
            database.get_database_backend(),
            "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
        ))
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::Database("数据库时钟查询没有返回记录".into()))?;
    let value: chrono::NaiveDateTime = row.try_get("", "db_now").map_err(database_error)?;
    Ok(DateTime::from_naive_utc_and_offset(value, Utc))
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}
