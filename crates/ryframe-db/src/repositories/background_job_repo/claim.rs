use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, DatabaseConnection, EntityTrait, ExprTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait,
    sea_query::{LockBehavior, LockType},
};

use crate::{entities::background_job, repositories::ExecutionTenantFilter};

use super::{BackgroundJobRepository, database_error, validate_lease};

impl BackgroundJobRepository {
    /// 通过 `FOR UPDATE SKIP LOCKED` 领取一条可执行任务。
    ///
    /// 这是进入 `running` 的唯一状态迁移；行锁和 `attempts` 自增位于同一事务中。
    /// 因而进程在提交后崩溃时，任务会在独立的租约回收循环中再次投递。
    pub(crate) async fn claim_next(
        &self,
        db: &DatabaseConnection,
        worker_id: &str,
        lease_duration: Duration,
        now: DateTime<Utc>,
        tenant_scope: &ExecutionTenantFilter,
    ) -> AppResult<Option<background_job::Model>> {
        validate_lease(worker_id, lease_duration)?;
        let txn = db.begin().await.map_err(database_error)?;

        let Some(job) = Self::claimable_query(now, tenant_scope)
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
            .one(&txn)
            .await
            .map_err(database_error)?
        else {
            txn.commit().await.map_err(database_error)?;
            return Ok(None);
        };

        let attempts = job
            .attempts
            .checked_add(1)
            .ok_or_else(|| AppError::Database("background job attempts overflowed".into()))?;
        let mut active: background_job::ActiveModel = job.into();
        active.status = Set(background_job::Model::STATUS_RUNNING.to_owned());
        active.attempts = Set(attempts);
        active.lease_owner = Set(Some(worker_id.to_owned()));
        active.lease_until = Set(Some(now + lease_duration));
        active.updated_at = Set(now);
        active.completed_at = Set(None);
        let claimed = active.update(&txn).await.map_err(database_error)?;

        txn.commit().await.map_err(database_error)?;
        Ok(Some(claimed))
    }

    fn claimable_query(
        now: DateTime<Utc>,
        tenant_scope: &ExecutionTenantFilter,
    ) -> sea_orm::Select<background_job::Entity> {
        let mut query = background_job::Entity::find()
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_PENDING))
            .filter(background_job::Column::AvailableAt.lte(now))
            .filter(
                sea_orm::sea_query::Expr::col(background_job::Column::Attempts).lt(
                    sea_orm::sea_query::Expr::col(background_job::Column::MaxAttempts),
                ),
            )
            .order_by_desc(background_job::Column::Priority)
            .order_by_asc(background_job::Column::AvailableAt)
            .order_by_asc(background_job::Column::Id);
        if let Some(condition) = tenant_scope.condition(background_job::Column::TenantId) {
            query = query.filter(condition);
        }
        query
    }
}
