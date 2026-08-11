use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, Value};

#[derive(Clone, Debug)]
pub struct OverviewTrendCount {
    pub bucket_index: usize,
    pub dimension: String,
    pub count: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScheduleOverviewStats {
    pub enabled: u64,
    pub lag_seconds: f64,
}

pub struct OverviewRepository;

impl OverviewRepository {
    pub async fn background_job_trends(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        include_platform: bool,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket_seconds: u32,
    ) -> AppResult<Vec<OverviewTrendCount>> {
        self.query_trends(
            db,
            r#"
SELECT
    CAST(FLOOR(TIMESTAMPDIFF(MICROSECOND, ?, `created_at`) / (? * 1000000)) AS SIGNED) AS bucket_index,
    '' AS dimension,
    COUNT(*) AS item_count
FROM `sys_background_job`
WHERE `created_at` >= ? AND `created_at` < ?
  AND (`tenant_id` = ? OR (? = 1 AND `tenant_id` IS NULL))
GROUP BY bucket_index
ORDER BY bucket_index
"#,
            trend_values(
                start,
                end,
                bucket_seconds,
                [
                    Value::from(tenant_id.to_owned()),
                    Value::from(i32::from(include_platform)),
                ],
            ),
        )
        .await
    }

    pub async fn schedule_execution_trends(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket_seconds: u32,
    ) -> AppResult<Vec<OverviewTrendCount>> {
        self.query_trends(
            db,
            r#"
SELECT
    CAST(FLOOR(TIMESTAMPDIFF(MICROSECOND, ?, `created_at`) / (? * 1000000)) AS SIGNED) AS bucket_index,
    `outcome` AS dimension,
    COUNT(*) AS item_count
FROM `sys_job_schedule_execution`
WHERE `created_at` >= ? AND `created_at` < ? AND `tenant_id` = ?
GROUP BY bucket_index, `outcome`
ORDER BY bucket_index, `outcome`
"#,
            trend_values(
                start,
                end,
                bucket_seconds,
                [Value::from(tenant_id.to_owned())],
            ),
        )
        .await
    }

    pub async fn login_trends(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket_seconds: u32,
    ) -> AppResult<Vec<OverviewTrendCount>> {
        self.query_trends(
            db,
            r#"
SELECT
    CAST(FLOOR(TIMESTAMPDIFF(MICROSECOND, ?, `login_time`) / (? * 1000000)) AS SIGNED) AS bucket_index,
    `status` AS dimension,
    COUNT(*) AS item_count
FROM `sys_login_info`
WHERE `login_time` >= ? AND `login_time` < ? AND `tenant_id` = ?
GROUP BY bucket_index, `status`
ORDER BY bucket_index, `status`
"#,
            trend_values(
                start,
                end,
                bucket_seconds,
                [Value::from(tenant_id.to_owned())],
            ),
        )
        .await
    }

    pub async fn operation_trends(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        bucket_seconds: u32,
    ) -> AppResult<Vec<OverviewTrendCount>> {
        self.query_trends(
            db,
            r#"
SELECT
    CAST(FLOOR(TIMESTAMPDIFF(MICROSECOND, ?, `oper_time`) / (? * 1000000)) AS SIGNED) AS bucket_index,
    `status` AS dimension,
    COUNT(*) AS item_count
FROM `sys_oper_log`
WHERE `oper_time` >= ? AND `oper_time` < ? AND `tenant_id` = ?
GROUP BY bucket_index, `status`
ORDER BY bucket_index, `status`
"#,
            trend_values(
                start,
                end,
                bucket_seconds,
                [Value::from(tenant_id.to_owned())],
            ),
        )
        .await
    }

    pub async fn schedule_stats(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<ScheduleOverviewStats> {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                r#"
SELECT
    CAST(COALESCE(SUM(CASE WHEN `enabled` = 1 AND `del_flag` = '0' THEN 1 ELSE 0 END), 0) AS SIGNED) AS enabled_count,
    CAST(
        COALESCE(
            MAX(
                CASE
                    WHEN `enabled` = 1 AND `del_flag` = '0' AND `next_run_at` < ?
                    THEN TIMESTAMPDIFF(MICROSECOND, `next_run_at`, ?)
                    ELSE 0
                END
            ),
            0
        ) AS SIGNED
    ) AS lag_microseconds
FROM `sys_job_schedule`
WHERE `tenant_id` = ?
"#,
                [
                    Value::from(now),
                    Value::from(now),
                    Value::from(tenant_id.to_owned()),
                ],
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("运维总览调度统计没有返回记录".into()))?;
        let enabled = read_count(&row, "enabled_count")?;
        let lag_microseconds = read_count(&row, "lag_microseconds")?;
        Ok(ScheduleOverviewStats {
            enabled,
            lag_seconds: lag_microseconds as f64 / 1_000_000.0,
        })
    }

    async fn query_trends(
        &self,
        db: &DatabaseConnection,
        sql: &'static str,
        values: Vec<Value>,
    ) -> AppResult<Vec<OverviewTrendCount>> {
        db.query_all_raw(Statement::from_sql_and_values(
            db.get_database_backend(),
            sql,
            values,
        ))
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|row| {
            let bucket_index: i64 = row.try_get("", "bucket_index").map_err(database_error)?;
            let bucket_index = usize::try_from(bucket_index)
                .map_err(|_| AppError::Database("运维趋势时间桶索引无效".into()))?;
            let dimension = row
                .try_get::<String>("", "dimension")
                .map_err(database_error)?;
            Ok(OverviewTrendCount {
                bucket_index,
                dimension,
                count: read_count(&row, "item_count")?,
            })
        })
        .collect()
    }
}

fn trend_values<const N: usize>(
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    bucket_seconds: u32,
    filters: [Value; N],
) -> Vec<Value> {
    let mut values = vec![
        Value::from(start),
        Value::from(i64::from(bucket_seconds)),
        Value::from(start),
        Value::from(end),
    ];
    values.extend(filters);
    values
}

fn read_count(row: &sea_orm::QueryResult, column: &str) -> AppResult<u64> {
    let value: i64 = row.try_get("", column).map_err(database_error)?;
    u64::try_from(value).map_err(|_| AppError::Database(format!("{column} 统计结果无效")))
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
