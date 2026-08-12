use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ryframe_core::repository::{PageResult, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect, Statement, TransactionTrait, TryGetable,
    sea_query::{Expr, LockType},
};

use crate::entities::data_retention_run;

/// 数据保留支持的低基数资源集合。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RetentionResource {
    BackgroundJobs,
    OutboxEvents,
    ScheduleExecutions,
    ExportJobs,
    OperationLogs,
    LoginLogs,
    UserImports,
    RetentionRuns,
}

impl RetentionResource {
    pub const ALL: [Self; 8] = [
        Self::BackgroundJobs,
        Self::OutboxEvents,
        Self::ScheduleExecutions,
        Self::ExportJobs,
        Self::OperationLogs,
        Self::LoginLogs,
        Self::UserImports,
        Self::RetentionRuns,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::BackgroundJobs => "background_jobs",
            Self::OutboxEvents => "outbox_events",
            Self::ScheduleExecutions => "schedule_executions",
            Self::ExportJobs => "export_jobs",
            Self::OperationLogs => "operation_logs",
            Self::LoginLogs => "login_logs",
            Self::UserImports => "user_imports",
            Self::RetentionRuns => "retention_runs",
        }
    }

    const fn table(self) -> &'static str {
        match self {
            Self::BackgroundJobs => "sys_background_job",
            Self::OutboxEvents => "sys_outbox_event",
            Self::ScheduleExecutions => "sys_job_schedule_execution",
            Self::ExportJobs => "sys_export_job",
            Self::OperationLogs => "sys_oper_log",
            Self::LoginLogs => "sys_login_info",
            Self::UserImports => "sys_user_import_job",
            Self::RetentionRuns => "sys_data_retention_run",
        }
    }

    const fn predicate(self) -> &'static str {
        match self {
            Self::BackgroundJobs => "status = 'succeeded' AND completed_at < ?",
            Self::OutboxEvents => "status = 'published' AND published_at < ?",
            Self::ScheduleExecutions => {
                "created_at < ? AND (background_job_id IS NULL OR NOT EXISTS (SELECT 1 FROM sys_background_job linked_job WHERE linked_job.id = sys_job_schedule_execution.background_job_id AND linked_job.status <> 'succeeded'))"
            }
            Self::ExportJobs => {
                "status IN ('succeeded', 'failed', 'cancelled', 'expired') AND completed_at < ?"
            }
            Self::OperationLogs => "oper_time < ?",
            Self::LoginLogs => "login_time < ?",
            Self::UserImports => {
                "status IN ('succeeded', 'partial', 'failed', 'cancelled') AND completed_at < ?"
            }
            Self::RetentionRuns => {
                "status IN ('succeeded', 'partial', 'failed') AND completed_at < ?"
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RetentionCutoff {
    pub resource: RetentionResource,
    pub before: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug)]
pub struct RetentionCleanupResult {
    pub deleted: u64,
    pub remaining: u64,
}

pub struct DataRetentionRepository;

impl DataRetentionRepository {
    pub async fn database_utc_now<C>(&self, db: &C) -> AppResult<DateTime<Utc>>
    where
        C: ConnectionTrait,
    {
        let row = db
            .query_one_raw(Statement::from_string(
                db.get_database_backend(),
                "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("数据库时钟查询没有返回记录".into()))?;
        let value: chrono::NaiveDateTime = row.try_get("", "db_now").map_err(database_error)?;
        Ok(DateTime::from_naive_utc_and_offset(value, Utc))
    }

    pub async fn insert_run_if_missing<C>(
        &self,
        db: &C,
        model: data_retention_run::Model,
    ) -> AppResult<data_retention_run::Model>
    where
        C: ConnectionTrait,
    {
        let existing = data_retention_run::Entity::find()
            .filter(data_retention_run::Column::BackgroundJobId.eq(model.background_job_id))
            .one(db)
            .await
            .map_err(database_error)?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        match data_retention_run::ActiveModel::from(model.clone())
            .insert(db)
            .await
        {
            Ok(inserted) => Ok(inserted),
            Err(error) if error.to_string().contains("Duplicate") => {
                data_retention_run::Entity::find()
                    .filter(data_retention_run::Column::BackgroundJobId.eq(model.background_job_id))
                    .one(db)
                    .await
                    .map_err(database_error)?
                    .ok_or_else(|| AppError::Conflict("数据保留运行记录正在创建".into()))
            }
            Err(error) => Err(database_error(error)),
        }
    }

    pub async fn find_run_by_background_job<C>(
        &self,
        db: &C,
        background_job_id: i64,
    ) -> AppResult<Option<data_retention_run::Model>>
    where
        C: ConnectionTrait,
    {
        data_retention_run::Entity::find()
            .filter(data_retention_run::Column::BackgroundJobId.eq(background_job_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 锁定后台任务对应的运行记录，供执行器以幂等方式确认是否仍需清理。
    pub async fn lock_run_by_background_job_in_txn(
        &self,
        transaction: &sea_orm::DatabaseTransaction,
        background_job_id: i64,
    ) -> AppResult<Option<data_retention_run::Model>> {
        data_retention_run::Entity::find()
            .filter(data_retention_run::Column::BackgroundJobId.eq(background_job_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    /// 在锁定事务中把可恢复状态切换为运行中；完成态保持不变并返回 `false`。
    pub async fn begin_run_in_txn(
        &self,
        transaction: &sea_orm::DatabaseTransaction,
        mut model: data_retention_run::Model,
        now: DateTime<Utc>,
    ) -> AppResult<Option<data_retention_run::Model>> {
        if matches!(
            model.status.as_str(),
            data_retention_run::Model::STATUS_SUCCEEDED | data_retention_run::Model::STATUS_PARTIAL
        ) {
            return Ok(None);
        }
        model.status = data_retention_run::Model::STATUS_RUNNING.to_owned();
        model.started_at.get_or_insert(now);
        model.completed_at = None;
        model.error_summary = None;
        model.updated_at = now;
        let saved = data_retention_run::ActiveModel::from(model)
            .reset_all()
            .update(transaction)
            .await
            .map_err(database_error)?;
        Ok(Some(saved))
    }

    pub async fn list_runs(
        &self,
        db: &DatabaseConnection,
        page: &ValidatedPageQuery,
    ) -> AppResult<PageResult<data_retention_run::Model>> {
        let query = data_retention_run::Entity::find()
            .order_by_desc(data_retention_run::Column::CreatedAt)
            .order_by_desc(data_retention_run::Column::Id);
        crate::pagination::paginate(db, query, page).await
    }

    pub async fn update_run(
        &self,
        db: &DatabaseConnection,
        model: data_retention_run::Model,
    ) -> AppResult<data_retention_run::Model> {
        let id = model.id;
        let result = data_retention_run::Entity::update_many()
            .col_expr(
                data_retention_run::Column::Status,
                Expr::value(model.status),
            )
            .col_expr(
                data_retention_run::Column::PolicySnapshot,
                Expr::value(model.policy_snapshot),
            )
            .col_expr(
                data_retention_run::Column::EligibleCounts,
                Expr::value(model.eligible_counts),
            )
            .col_expr(
                data_retention_run::Column::DeletedCounts,
                Expr::value(model.deleted_counts),
            )
            .col_expr(
                data_retention_run::Column::RemainingCounts,
                Expr::value(model.remaining_counts),
            )
            .col_expr(
                data_retention_run::Column::ErrorSummary,
                Expr::value(model.error_summary),
            )
            .col_expr(
                data_retention_run::Column::StartedAt,
                Expr::value(model.started_at),
            )
            .col_expr(
                data_retention_run::Column::CompletedAt,
                Expr::value(model.completed_at),
            )
            .col_expr(
                data_retention_run::Column::UpdatedAt,
                Expr::value(model.updated_at),
            )
            .filter(data_retention_run::Column::Id.eq(id))
            .exec(db)
            .await
            .map_err(database_error)?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("数据保留运行记录不存在".into()));
        }
        data_retention_run::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("数据保留运行记录不存在".into()))
    }

    pub async fn preview(
        &self,
        db: &DatabaseConnection,
        cutoffs: &[RetentionCutoff],
        current_run_id: Option<i64>,
    ) -> AppResult<BTreeMap<String, u64>> {
        let mut counts = BTreeMap::new();
        for cutoff in cutoffs {
            counts.insert(
                cutoff.resource.key().to_owned(),
                self.count_eligible(db, *cutoff, current_run_id).await?,
            );
        }
        Ok(counts)
    }

    pub async fn cleanup_resource(
        &self,
        db: &DatabaseConnection,
        cutoff: RetentionCutoff,
        batch_size: usize,
        maximum: usize,
        current_run_id: Option<i64>,
    ) -> AppResult<RetentionCleanupResult> {
        let mut deleted = 0_u64;
        while deleted < maximum as u64 {
            let remaining_limit = (maximum as u64 - deleted) as usize;
            let limit = batch_size.min(remaining_limit);
            let transaction = db.begin().await.map_err(database_error)?;
            let (condition, mut values) = condition_and_values(cutoff, current_run_id);
            values.push((limit as u64).into());
            let rows = transaction
                .query_all_raw(Statement::from_sql_and_values(
                    transaction.get_database_backend(),
                    format!(
                        "SELECT id FROM `{}` WHERE {condition} ORDER BY id LIMIT ? FOR UPDATE SKIP LOCKED",
                        cutoff.resource.table()
                    ),
                    values,
                ))
                .await
                .map_err(database_error)?;
            let ids = rows
                .iter()
                .map(|row| i64::try_get_by_index(row, 0).map_err(try_get_error))
                .collect::<AppResult<Vec<_>>>()?;
            if ids.is_empty() {
                transaction.commit().await.map_err(database_error)?;
                break;
            }
            let placeholders = std::iter::repeat_n("?", ids.len())
                .collect::<Vec<_>>()
                .join(", ");
            transaction
                .execute_raw(Statement::from_sql_and_values(
                    transaction.get_database_backend(),
                    format!(
                        "DELETE FROM `{}` WHERE id IN ({placeholders})",
                        cutoff.resource.table()
                    ),
                    ids.iter().copied().map(Into::into),
                ))
                .await
                .map_err(database_error)?;
            transaction.commit().await.map_err(database_error)?;
            deleted += ids.len() as u64;
            if ids.len() < limit {
                break;
            }
        }
        let remaining = self.count_eligible(db, cutoff, current_run_id).await?;
        Ok(RetentionCleanupResult { deleted, remaining })
    }

    async fn count_eligible(
        &self,
        db: &DatabaseConnection,
        cutoff: RetentionCutoff,
        current_run_id: Option<i64>,
    ) -> AppResult<u64> {
        let (condition, values) = condition_and_values(cutoff, current_run_id);
        let row = db
            .query_one_raw(Statement::from_sql_and_values(
                db.get_database_backend(),
                format!(
                    "SELECT COUNT(*) FROM `{}` WHERE {condition}",
                    cutoff.resource.table()
                ),
                values,
            ))
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::Database("数据保留统计没有返回记录".into()))?;
        let count = i64::try_get_by_index(&row, 0).map_err(try_get_error)?;
        u64::try_from(count).map_err(|_| AppError::Database("数据保留统计结果无效".into()))
    }
}

fn condition_and_values(
    cutoff: RetentionCutoff,
    current_run_id: Option<i64>,
) -> (String, Vec<sea_orm::Value>) {
    let mut condition = cutoff.resource.predicate().to_owned();
    let mut values = vec![cutoff.before.naive_utc().into()];
    if cutoff.resource == RetentionResource::RetentionRuns
        && let Some(current_run_id) = current_run_id
    {
        condition.push_str(" AND id <> ?");
        values.push(current_run_id.into());
    }
    (condition, values)
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

fn try_get_error(error: sea_orm::TryGetError) -> AppError {
    AppError::Database(format!("{error:?}"))
}
