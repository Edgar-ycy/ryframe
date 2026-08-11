use chrono::{DateTime, Utc};
use ryframe_core::repository::{PageResult, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, DatabaseConnection, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    sea_query::{LikeExpr, LockBehavior, LockType},
};

use crate::entities::{background_job, job_schedule, job_schedule_execution};

#[derive(Clone, Debug, Default)]
pub struct JobScheduleFilter<'a> {
    pub name: Option<&'a str>,
    pub handler_key: Option<&'a str>,
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct JobScheduleExecutionFilter<'a> {
    pub trigger_kind: Option<&'a str>,
    pub outcome: Option<&'a str>,
    pub background_job_status: Option<&'a str>,
}

/// 调度计划及执行历史仓储。
pub struct JobScheduleRepository;

impl JobScheduleRepository {
    pub async fn list(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        filter: JobScheduleFilter<'_>,
        page: &ValidatedPageQuery,
    ) -> AppResult<PageResult<job_schedule::Model>> {
        let mut query = job_schedule::Entity::find()
            .filter(job_schedule::Column::TenantId.eq(tenant_id))
            .filter(job_schedule::Column::DelFlag.eq(job_schedule::Model::DEL_FLAG_NORMAL));
        if let Some(name) = filter.name {
            let escaped = name
                .replace('!', "!!")
                .replace('%', "!%")
                .replace('_', "!_");
            query = query.filter(
                job_schedule::Column::Name.like(LikeExpr::new(format!("%{escaped}%")).escape('!')),
            );
        }
        if let Some(handler_key) = filter.handler_key {
            query = query.filter(job_schedule::Column::HandlerKey.eq(handler_key));
        }
        if let Some(enabled) = filter.enabled {
            query = query.filter(job_schedule::Column::Enabled.eq(enabled));
        }
        crate::pagination::paginate(
            db,
            query
                .order_by_desc(job_schedule::Column::CreatedAt)
                .order_by_desc(job_schedule::Column::Id),
            page,
        )
        .await
    }

    pub async fn find_for_tenant(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<job_schedule::Model>> {
        job_schedule::Entity::find_by_id(id)
            .filter(job_schedule::Column::TenantId.eq(tenant_id))
            .filter(job_schedule::Column::DelFlag.eq(job_schedule::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn lock_for_tenant(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<job_schedule::Model>> {
        job_schedule::Entity::find_by_id(id)
            .filter(job_schedule::Column::TenantId.eq(tenant_id))
            .filter(job_schedule::Column::DelFlag.eq(job_schedule::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn lock_next_due(
        &self,
        transaction: &DatabaseTransaction,
        now: DateTime<Utc>,
    ) -> AppResult<Option<job_schedule::Model>> {
        job_schedule::Entity::find()
            .filter(job_schedule::Column::Enabled.eq(true))
            .filter(job_schedule::Column::DelFlag.eq(job_schedule::Model::DEL_FLAG_NORMAL))
            .filter(
                Condition::any()
                    .add(job_schedule::Column::NextRunAt.is_null())
                    .add(job_schedule::Column::NextRunAt.lte(now)),
            )
            .order_by_asc(job_schedule::Column::NextRunAt)
            .order_by_asc(job_schedule::Column::Id)
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    pub async fn count_enabled<C>(&self, db: &C, tenant_id: &str) -> AppResult<u64>
    where
        C: sea_orm::ConnectionTrait,
    {
        job_schedule::Entity::find()
            .filter(job_schedule::Column::TenantId.eq(tenant_id))
            .filter(job_schedule::Column::Enabled.eq(true))
            .filter(job_schedule::Column::DelFlag.eq(job_schedule::Model::DEL_FLAG_NORMAL))
            .count(db)
            .await
            .map_err(database_error)
    }

    pub async fn has_active_job<C>(&self, db: &C, schedule_id: i64) -> AppResult<bool>
    where
        C: sea_orm::ConnectionTrait,
    {
        background_job::Entity::find()
            .filter(background_job::Column::ScheduleId.eq(schedule_id))
            .filter(
                Condition::any()
                    .add(background_job::Column::Status.eq(background_job::Model::STATUS_PENDING))
                    .add(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING)),
            )
            .one(db)
            .await
            .map(|job| job.is_some())
            .map_err(database_error)
    }

    pub async fn insert<C>(
        &self,
        db: &C,
        active: job_schedule::ActiveModel,
    ) -> AppResult<job_schedule::Model>
    where
        C: sea_orm::ConnectionTrait,
    {
        active.insert(db).await.map_err(database_error)
    }

    pub async fn find_execution_by_fire_key<C>(
        &self,
        db: &C,
        schedule_id: i64,
        fire_key: &str,
    ) -> AppResult<Option<job_schedule_execution::Model>>
    where
        C: sea_orm::ConnectionTrait,
    {
        job_schedule_execution::Entity::find()
            .filter(job_schedule_execution::Column::ScheduleId.eq(schedule_id))
            .filter(job_schedule_execution::Column::FireKey.eq(fire_key))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn list_executions(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        schedule_id: i64,
        filter: JobScheduleExecutionFilter<'_>,
        page: &ValidatedPageQuery,
    ) -> AppResult<PageResult<job_schedule_execution::Model>> {
        let mut query = job_schedule_execution::Entity::find()
            .filter(job_schedule_execution::Column::TenantId.eq(tenant_id))
            .filter(job_schedule_execution::Column::ScheduleId.eq(schedule_id));
        if let Some(trigger_kind) = filter.trigger_kind {
            query = query.filter(job_schedule_execution::Column::TriggerKind.eq(trigger_kind));
        }
        if let Some(outcome) = filter.outcome {
            query = query.filter(job_schedule_execution::Column::Outcome.eq(outcome));
        }
        if let Some(status) = filter.background_job_status {
            query = query
                .left_join(background_job::Entity)
                .filter(background_job::Column::Status.eq(status));
        }
        crate::pagination::paginate(
            db,
            query
                .order_by_desc(job_schedule_execution::Column::CreatedAt)
                .order_by_desc(job_schedule_execution::Column::Id),
            page,
        )
        .await
    }

    pub async fn background_job_statuses(
        &self,
        db: &DatabaseConnection,
        ids: &[i64],
    ) -> AppResult<std::collections::HashMap<i64, String>> {
        if ids.is_empty() {
            return Ok(std::collections::HashMap::new());
        }
        let jobs = background_job::Entity::find()
            .filter(background_job::Column::Id.is_in(ids.iter().copied()))
            .all(db)
            .await
            .map_err(database_error)?;
        Ok(jobs.into_iter().map(|job| (job.id, job.status)).collect())
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
