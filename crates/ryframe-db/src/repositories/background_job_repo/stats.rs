use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, EntityTrait, ExprTrait,
    QueryFilter, QueryOrder, QueryResult, Statement, Value, sea_query::Expr,
};

use crate::{ExecutionTenantScope, entities::background_job};

use super::{
    BackgroundJobFilter, BackgroundJobRepository, BackgroundJobStats, BackgroundJobTypeStats,
    database_error,
};

const QUEUE_STATS_SQL: &str = r#"
SELECT
    COUNT(*) AS total,
    CAST(COALESCE(SUM(CASE WHEN `status` = ? THEN 1 ELSE 0 END), 0) AS SIGNED) AS pending,
    CAST(COALESCE(SUM(CASE WHEN `status` = ? THEN 1 ELSE 0 END), 0) AS SIGNED) AS running,
    CAST(COALESCE(SUM(CASE WHEN `status` = ? THEN 1 ELSE 0 END), 0) AS SIGNED) AS succeeded,
    CAST(COALESCE(SUM(CASE WHEN `status` = ? THEN 1 ELSE 0 END), 0) AS SIGNED) AS dead,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN `status` = ?
                        AND `available_at` <= ?
                        AND `attempts` < `max_attempts`
                    THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS SIGNED
    ) AS ready
FROM `sys_background_job`
"#;

const TYPE_STATS_SQL_PREFIX: &str = r#"
SELECT
    `job_type`,
    CAST(COALESCE(SUM(CASE WHEN `status` = ? THEN 1 ELSE 0 END), 0) AS SIGNED) AS pending,
    CAST(COALESCE(SUM(CASE WHEN `status` = ? THEN 1 ELSE 0 END), 0) AS SIGNED) AS running,
    CAST(COALESCE(SUM(CASE WHEN `status` = ? THEN 1 ELSE 0 END), 0) AS SIGNED) AS dead,
    CAST(
        COALESCE(
            SUM(
                CASE
                    WHEN `status` = ?
                        AND `available_at` <= UTC_TIMESTAMP(6)
                        AND `attempts` < `max_attempts`
                    THEN 1
                    ELSE 0
                END
            ),
            0
        ) AS SIGNED
    ) AS ready,
    CAST(
        GREATEST(
            0,
            COALESCE(
                TIMESTAMPDIFF(
                    MICROSECOND,
                    MIN(
                        CASE
                            WHEN `status` = ?
                                AND `available_at` <= UTC_TIMESTAMP(6)
                                AND `attempts` < `max_attempts`
                            THEN `available_at`
                        END
                    ),
                    UTC_TIMESTAMP(6)
                ),
                0
            )
        ) AS SIGNED
    ) AS oldest_ready_age_microseconds
FROM `sys_background_job`
WHERE `job_type` IN (
"#;

impl BackgroundJobRepository {
    pub async fn list(
        &self,
        db: &DatabaseConnection,
        filter: BackgroundJobFilter<'_>,
        query: &ryframe_adapters::repository::ValidatedPageQuery,
    ) -> AppResult<ryframe_adapters::repository::PageResult<background_job::Model>> {
        crate::pagination::paginate(
            db,
            Self::filtered_query(filter)
                .order_by_desc(background_job::Column::CreatedAt)
                .order_by_desc(background_job::Column::Id),
            query,
        )
        .await
    }

    pub async fn stats(
        &self,
        db: &DatabaseConnection,
        now: DateTime<Utc>,
    ) -> AppResult<BackgroundJobStats> {
        self.stats_filtered(db, BackgroundJobFilter::default(), now)
            .await
    }

    /// 按筛选范围统计队列状态。监控接口应始终提供当前租户过滤条件。
    pub async fn stats_filtered(
        &self,
        db: &DatabaseConnection,
        filter: BackgroundJobFilter<'_>,
        now: DateTime<Utc>,
    ) -> AppResult<BackgroundJobStats> {
        let (filter_sql, filter_values) = queue_stats_filter(filter);
        let mut sql = QUEUE_STATS_SQL.to_owned();
        if !filter_sql.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&filter_sql);
        }
        let mut values = vec![
            Value::from(background_job::Model::STATUS_PENDING.to_owned()),
            Value::from(background_job::Model::STATUS_RUNNING.to_owned()),
            Value::from(background_job::Model::STATUS_SUCCEEDED.to_owned()),
            Value::from(background_job::Model::STATUS_DEAD.to_owned()),
            Value::from(background_job::Model::STATUS_PENDING.to_owned()),
            Value::from(now),
        ];
        values.extend(filter_values);
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                sql,
                values,
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| {
                AppError::Database("background job statistics query returned no row".into())
            })?;
        Ok(BackgroundJobStats {
            total: read_count(&row, "total")?,
            pending: read_count(&row, "pending")?,
            running: read_count(&row, "running")?,
            succeeded: read_count(&row, "succeeded")?,
            dead: read_count(&row, "dead")?,
            ready: read_count(&row, "ready")?,
        })
    }

    /// 使用一条条件聚合查询读取所有已注册任务类型的监控指标。
    ///
    /// 数据库中尚无记录的注册类型也会返回零值，避免 Prometheus 保留旧 Gauge。
    pub async fn stats_for_types(
        &self,
        db: &DatabaseConnection,
        job_types: &[String],
        tenant_scope: &ExecutionTenantScope,
    ) -> AppResult<Vec<BackgroundJobTypeStats>> {
        let job_types = unique_job_types(job_types);
        if job_types.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = vec!["?"; job_types.len()].join(", ");
        let mut sql = String::with_capacity(TYPE_STATS_SQL_PREFIX.len() + placeholders.len() + 32);
        sql.push_str(TYPE_STATS_SQL_PREFIX);
        sql.push_str(&placeholders);
        sql.push(')');
        if tenant_scope.tenant_id().is_some() {
            sql.push_str(" AND (`tenant_id` = ? OR `tenant_id` IS NULL)");
        }
        sql.push_str(" GROUP BY `job_type`");
        let mut values = vec![
            Value::from(background_job::Model::STATUS_PENDING.to_owned()),
            Value::from(background_job::Model::STATUS_RUNNING.to_owned()),
            Value::from(background_job::Model::STATUS_DEAD.to_owned()),
            Value::from(background_job::Model::STATUS_PENDING.to_owned()),
            Value::from(background_job::Model::STATUS_PENDING.to_owned()),
        ];
        values.extend(job_types.iter().cloned().map(Value::from));
        if let Some(tenant_id) = tenant_scope.tenant_id() {
            values.push(Value::from(tenant_id.to_owned()));
        }

        let rows = db
            .query_all_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                sql,
                values,
            ))
            .await
            .map_err(database_error)?;
        let mut grouped = std::collections::BTreeMap::new();
        for row in rows {
            let job_type: String = row.try_get("", "job_type").map_err(database_error)?;
            let ready = read_count(&row, "ready")?;
            let oldest_ready_age = if ready == 0 {
                None
            } else {
                Some(StdDuration::from_micros(read_count(
                    &row,
                    "oldest_ready_age_microseconds",
                )?))
            };
            grouped.insert(
                job_type.clone(),
                BackgroundJobTypeStats {
                    job_type,
                    pending: read_count(&row, "pending")?,
                    running: read_count(&row, "running")?,
                    dead: read_count(&row, "dead")?,
                    ready,
                    oldest_ready_age,
                },
            );
        }

        Ok(job_types
            .into_iter()
            .map(|job_type| {
                grouped.remove(&job_type).unwrap_or(BackgroundJobTypeStats {
                    job_type,
                    ..Default::default()
                })
            })
            .collect())
    }

    /// 返回筛选范围内最早可执行任务的等待时长；没有可执行任务时返回 `None`。
    pub async fn oldest_ready_age(
        &self,
        db: &DatabaseConnection,
        filter: BackgroundJobFilter<'_>,
        now: DateTime<Utc>,
    ) -> AppResult<Option<StdDuration>> {
        let job = Self::filtered_query(filter)
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_PENDING))
            .filter(background_job::Column::AvailableAt.lte(now))
            .filter(
                Expr::col(background_job::Column::Attempts)
                    .lt(Expr::col(background_job::Column::MaxAttempts)),
            )
            .order_by_asc(background_job::Column::AvailableAt)
            .order_by_asc(background_job::Column::Id)
            .one(db)
            .await
            .map_err(database_error)?;
        Ok(job.map(|job| {
            (now - job.available_at)
                .to_std()
                .unwrap_or(StdDuration::ZERO)
        }))
    }

    fn filtered_query(filter: BackgroundJobFilter<'_>) -> sea_orm::Select<background_job::Entity> {
        let mut select = background_job::Entity::find();
        if let Some(tenant_id) = filter.tenant_id {
            select = if filter.include_platform {
                select.filter(
                    Condition::any()
                        .add(background_job::Column::TenantId.eq(tenant_id))
                        .add(background_job::Column::TenantId.is_null()),
                )
            } else {
                select.filter(background_job::Column::TenantId.eq(tenant_id))
            };
        }
        if let Some(schedule_id) = filter.schedule_id {
            select = select.filter(background_job::Column::ScheduleId.eq(schedule_id));
        }
        if let Some(job_type) = filter.job_type {
            select = select.filter(background_job::Column::JobType.eq(job_type));
        }
        if let Some(status) = filter.status {
            select = select.filter(background_job::Column::Status.eq(status));
        }
        select
    }
}

fn queue_stats_filter(filter: BackgroundJobFilter<'_>) -> (String, Vec<Value>) {
    let mut conditions = Vec::with_capacity(5);
    let mut values = Vec::with_capacity(4);
    if let Some(tenant_id) = filter.tenant_id {
        conditions.push(if filter.include_platform {
            "(`tenant_id` = ? OR `tenant_id` IS NULL)"
        } else {
            "`tenant_id` = ?"
        });
        values.push(Value::from(tenant_id.to_owned()));
    }
    if let Some(schedule_id) = filter.schedule_id {
        conditions.push("`schedule_id` = ?");
        values.push(Value::from(schedule_id));
    }
    if let Some(job_type) = filter.job_type {
        conditions.push("`job_type` = ?");
        values.push(Value::from(job_type.to_owned()));
    }
    if let Some(status) = filter.status {
        conditions.push("`status` = ?");
        values.push(Value::from(status.to_owned()));
    }
    (conditions.join(" AND "), values)
}

fn unique_job_types(job_types: &[String]) -> Vec<String> {
    job_types
        .iter()
        .filter(|job_type| !job_type.is_empty())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn read_count(row: &QueryResult, column: &str) -> AppResult<u64> {
    let value: i64 = row.try_get("", column).map_err(database_error)?;
    u64::try_from(value).map_err(|_| {
        AppError::Database(format!(
            "background job statistics column {column} contained a negative value"
        ))
    })
}
