use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::AppResult;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, ExprTrait, QueryFilter,
    QuerySelect, TransactionTrait,
    sea_query::{Expr, LockType},
};

use crate::entities::background_job;

use super::{
    BackgroundJobRepository, ExpiredLeaseRecovery, JobFailureDisposition, database_error,
    validate_lease,
};

/// 租约到期且尝试次数耗尽时写入的安全诊断原因。
const EXPIRED_LEASE_DEAD_ERROR: &str = "任务租约已过期，处理结果未知";

impl BackgroundJobRepository {
    /// 回收崩溃 Worker 遗留的过期租约。
    ///
    /// 任务被领取时即消耗一次尝试；最后一次已过期的租约直接进入 `dead`，其余任务
    /// 回到 `pending` 并立即可再次领取。先处理死信再重入队，避免耗尽任务被错误复活。
    pub async fn recover_expired_leases(
        &self,
        db: &DatabaseConnection,
        now: DateTime<Utc>,
    ) -> AppResult<ExpiredLeaseRecovery> {
        self.recover_expired_leases_on(db, now).await
    }

    async fn recover_expired_leases_on<C>(
        &self,
        db: &C,
        now: DateTime<Utc>,
    ) -> AppResult<ExpiredLeaseRecovery>
    where
        C: sea_orm::ConnectionTrait,
    {
        let dead = Self::expired_lease_dead_query(now)
            .exec(db)
            .await
            .map_err(database_error)?;
        let requeued = background_job::Entity::update_many()
            .col_expr(
                background_job::Column::Status,
                Expr::value(background_job::Model::STATUS_PENDING),
            )
            .col_expr(
                background_job::Column::LeaseOwner,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                background_job::Column::LeaseUntil,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .col_expr(background_job::Column::AvailableAt, Expr::value(now))
            .col_expr(background_job::Column::UpdatedAt, Expr::value(now))
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseUntil.lte(now))
            .filter(
                Expr::col(background_job::Column::Attempts)
                    .lt(Expr::col(background_job::Column::MaxAttempts)),
            )
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(ExpiredLeaseRecovery {
            requeued: requeued.rows_affected,
            dead: dead.rows_affected,
        })
    }

    /// 将仍由当前 Worker 持有且未过期的租约标记为成功。
    ///
    /// 返回 `false` 说明租约已失效或已被其他 Worker 接管，此时处理结果不能视为最终结果。
    pub async fn complete(
        &self,
        db: &DatabaseConnection,
        job_id: i64,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let result = background_job::Entity::update_many()
            .col_expr(
                background_job::Column::Status,
                Expr::value(background_job::Model::STATUS_SUCCEEDED),
            )
            .col_expr(
                background_job::Column::LeaseOwner,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                background_job::Column::LeaseUntil,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .col_expr(background_job::Column::UpdatedAt, Expr::value(now))
            .col_expr(background_job::Column::CompletedAt, Expr::value(now))
            .filter(background_job::Column::Id.eq(job_id))
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseOwner.eq(worker_id))
            .filter(background_job::Column::LeaseUntil.gt(now))
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 续期正在处理的租约。耗时任务应在原租约失效前调用；续期采用比较并交换语义。
    pub async fn renew_lease(
        &self,
        db: &DatabaseConnection,
        job_id: i64,
        worker_id: &str,
        lease_duration: Duration,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        validate_lease(worker_id, lease_duration)?;
        let result = background_job::Entity::update_many()
            .col_expr(
                background_job::Column::LeaseUntil,
                Expr::value(now + lease_duration),
            )
            .col_expr(background_job::Column::UpdatedAt, Expr::value(now))
            .filter(background_job::Column::Id.eq(job_id))
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseOwner.eq(worker_id))
            .filter(background_job::Column::LeaseUntil.gt(now))
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 完成一次失败处理：安排重试，或在尝试次数耗尽后标记为死信。
    pub async fn fail(
        &self,
        db: &DatabaseConnection,
        job_id: i64,
        worker_id: &str,
        retry_at: DateTime<Utc>,
        error_message: &str,
        now: DateTime<Utc>,
    ) -> AppResult<JobFailureDisposition> {
        let txn = db.begin().await.map_err(database_error)?;
        let Some(job) = Self::owned_running_query(job_id, worker_id)
            .lock(LockType::Update)
            .one(&txn)
            .await
            .map_err(database_error)?
        else {
            rollback_quietly(txn).await;
            return Ok(JobFailureDisposition::LeaseLost);
        };
        if job.lease_until.is_none_or(|lease_until| lease_until <= now) {
            rollback_quietly(txn).await;
            return Ok(JobFailureDisposition::LeaseLost);
        }

        let dead = job.attempts >= job.max_attempts;
        let mut active: background_job::ActiveModel = job.into();
        active.status = Set(if dead {
            background_job::Model::STATUS_DEAD
        } else {
            background_job::Model::STATUS_PENDING
        }
        .to_owned());
        active.available_at = Set(if dead { now } else { retry_at });
        active.lease_owner = Set(None);
        active.lease_until = Set(None);
        active.last_error = Set(Some(truncate_error(error_message)));
        active.updated_at = Set(now);
        active.completed_at = Set(dead.then_some(now));
        active.update(&txn).await.map_err(database_error)?;
        txn.commit().await.map_err(database_error)?;

        Ok(if dead {
            JobFailureDisposition::Dead
        } else {
            JobFailureDisposition::Retried {
                available_at: retry_at,
            }
        })
    }

    /// 当重试无法推进时显式将任务标记为死信（例如没有注册对应类型的处理器）。
    pub async fn dead_letter(
        &self,
        db: &DatabaseConnection,
        job_id: i64,
        worker_id: &str,
        error_message: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let result = background_job::Entity::update_many()
            .col_expr(
                background_job::Column::Status,
                Expr::value(background_job::Model::STATUS_DEAD),
            )
            .col_expr(
                background_job::Column::LeaseOwner,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                background_job::Column::LeaseUntil,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .col_expr(
                background_job::Column::LastError,
                Expr::value(Some(truncate_error(error_message))),
            )
            .col_expr(background_job::Column::UpdatedAt, Expr::value(now))
            .col_expr(background_job::Column::CompletedAt, Expr::value(now))
            .filter(background_job::Column::Id.eq(job_id))
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseOwner.eq(worker_id))
            .filter(background_job::Column::LeaseUntil.gt(now))
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 将当前租户的一条死信任务重新置为待执行状态。
    ///
    /// 人工重试代表开启新的尝试预算，因此会清空 `attempts` 和完成时间；保留
    /// `last_error` 以便排查上一次失败原因。条件更新可避免与其他管理操作相互覆盖。
    pub async fn retry_dead(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        include_platform: bool,
        job_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let result = Self::retry_dead_query(tenant_id, include_platform, job_id, now)
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    fn owned_running_query(
        job_id: i64,
        worker_id: &str,
    ) -> sea_orm::Select<background_job::Entity> {
        background_job::Entity::find_by_id(job_id)
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseOwner.eq(worker_id))
    }

    fn retry_dead_query(
        tenant_id: &str,
        include_platform: bool,
        job_id: i64,
        now: DateTime<Utc>,
    ) -> sea_orm::UpdateMany<background_job::Entity> {
        let query = background_job::Entity::update_many()
            .col_expr(
                background_job::Column::Status,
                Expr::value(background_job::Model::STATUS_PENDING),
            )
            .col_expr(background_job::Column::Attempts, Expr::value(0))
            .col_expr(background_job::Column::AvailableAt, Expr::value(now))
            .col_expr(
                background_job::Column::LeaseOwner,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                background_job::Column::LeaseUntil,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .col_expr(background_job::Column::UpdatedAt, Expr::value(now))
            .col_expr(
                background_job::Column::CompletedAt,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .filter(background_job::Column::Id.eq(job_id))
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_DEAD));
        if include_platform {
            query.filter(
                sea_orm::Condition::any()
                    .add(background_job::Column::TenantId.eq(tenant_id))
                    .add(background_job::Column::TenantId.is_null()),
            )
        } else {
            query.filter(background_job::Column::TenantId.eq(tenant_id))
        }
    }

    fn expired_lease_dead_query(now: DateTime<Utc>) -> sea_orm::UpdateMany<background_job::Entity> {
        background_job::Entity::update_many()
            .col_expr(
                background_job::Column::Status,
                Expr::value(background_job::Model::STATUS_DEAD),
            )
            .col_expr(
                background_job::Column::LeaseOwner,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                background_job::Column::LeaseUntil,
                Expr::value(Option::<DateTime<Utc>>::None),
            )
            .col_expr(
                background_job::Column::LastError,
                Expr::value(Some(EXPIRED_LEASE_DEAD_ERROR.to_owned())),
            )
            .col_expr(background_job::Column::UpdatedAt, Expr::value(now))
            .col_expr(background_job::Column::CompletedAt, Expr::value(now))
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseUntil.lte(now))
            .filter(
                Expr::col(background_job::Column::Attempts)
                    .gte(Expr::col(background_job::Column::MaxAttempts)),
            )
    }
}

fn truncate_error(error: &str) -> String {
    const MAX_ERROR_BYTES: usize = 8 * 1024;
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_owned();
    }
    let mut end = MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &error[..end])
}

async fn rollback_quietly(transaction: DatabaseTransaction) {
    if let Err(error) = transaction.rollback().await {
        tracing::warn!(error = %error, "failed to rollback background job lease transaction");
    }
}
