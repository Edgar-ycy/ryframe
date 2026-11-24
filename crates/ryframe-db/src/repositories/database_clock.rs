use crate::DbResultExt;
use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, Statement, TryGetable};
use time::{OffsetDateTime, PrimitiveDateTime};

/// 读取主数据库的 UTC 时钟，统一租约、过期和任务状态的时间基准。
pub async fn utc_now<C>(database: &C) -> AppResult<DateTime<Utc>>
where
    C: ConnectionTrait + ?Sized,
{
    let value = read_utc_now(database).await?;
    Ok(DateTime::from_naive_utc_and_offset(value, Utc))
}

pub(crate) async fn utc_now_time<C>(database: &C) -> AppResult<OffsetDateTime>
where
    C: ConnectionTrait + ?Sized,
{
    let value: PrimitiveDateTime = read_utc_now(database).await?;
    Ok(value.assume_utc())
}

pub(crate) fn utc_time_from_chrono(
    instant: DateTime<Utc>,
) -> Result<OffsetDateTime, time::error::ComponentRange> {
    ryframe_application::ports::utc_timestamp::from_chrono(instant)
}

async fn read_utc_now<C, T>(database: &C) -> AppResult<T>
where
    C: ConnectionTrait + ?Sized,
    T: TryGetable,
{
    let row = database
        .query_one_raw(Statement::from_string(
            database.get_database_backend(),
            "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
        ))
        .await
        .db()?
        .ok_or_else(|| AppError::Database("数据库时钟查询没有返回记录".into()))?;
    row.try_get("", "db_now").db()
}
