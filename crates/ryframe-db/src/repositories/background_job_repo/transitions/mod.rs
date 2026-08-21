use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::AppResult;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    sea_query::{Expr, LockType},
};

use crate::{
    entities::{
        background_job, data_retention_run, export_job, tenant_config_bundle,
        tenant_config_transfer, user_import_job,
    },
    repositories::{DataRetentionRepository, ExecutionTenantFilter},
};

use super::{
    BackgroundJobRepository, ExpiredLeaseRecovery, FailBackgroundJob, JobFailureDisposition,
    database_error, validate_lease,
};

/// 租约到期且尝试次数耗尽时写入的安全诊断原因。
const EXPIRED_LEASE_DEAD_ERROR: &str = "任务租约已过期，处理结果未知";

const USER_IMPORT_JOB_TYPE: &str = "system.user.import";
const EXPORT_JOB_TYPE: &str = "system.export.execute";
const DATA_RETENTION_JOB_TYPE: &str = "system.data_retention.cleanup";
const TENANT_CONFIG_EXPORT_JOB_TYPE: &str = "system.tenant_config.export";
const TENANT_CONFIG_PREVIEW_JOB_TYPE: &str = "system.tenant_config.preview";
const TENANT_CONFIG_APPLY_JOB_TYPE: &str = "system.tenant_config.apply";
const TENANT_CONFIG_ROLLBACK_JOB_TYPE: &str = "system.tenant_config.rollback";
const TENANT_CONFIG_EXPORT_SAFE_ERROR: &str = "配置包生成失败，请稍后重试或联系管理员";
const TENANT_CONFIG_PREVIEW_SAFE_ERROR: &str = "配置预览失败，请稍后重试或联系管理员";
const TENANT_CONFIG_APPLY_SAFE_ERROR: &str = "配置应用失败，请稍后重试或联系管理员";
const TENANT_CONFIG_ROLLBACK_SAFE_ERROR: &str = "配置回滚失败，请稍后重试或联系管理员";

pub(super) fn is_tenant_config_job(job_type: &str) -> bool {
    matches!(
        job_type,
        TENANT_CONFIG_EXPORT_JOB_TYPE
            | TENANT_CONFIG_PREVIEW_JOB_TYPE
            | TENANT_CONFIG_APPLY_JOB_TYPE
            | TENANT_CONFIG_ROLLBACK_JOB_TYPE
    )
}

pub(super) fn linked_resource_id(job: &background_job::Model, key: &str) -> Option<i64> {
    match job.payload.get(key)? {
        serde_json::Value::String(value) => value.parse().ok().filter(|id| *id > 0),
        serde_json::Value::Number(value) => value.as_i64().filter(|id| *id > 0),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(super) enum LinkedJobDisposition {
    Retried,
    Dead,
    ManuallyRetried,
}

mod config_transfer;
mod linked;
mod retention;
mod support;

use config_transfer::*;
use support::*;

impl BackgroundJobRepository {
    /// 在业务 control 事务内复活一条已关联的权威任务。
    ///
    /// 正在持有有效租约的 Worker 仍是唯一执行者，本方法不会抢占；其他状态
    /// 原地恢复为 pending 并重置尝试预算，不创建第二条可并发执行的任务。
    pub async fn reactivate_linked_in_txn<C>(
        &self,
        db: &C,
        job_id: i64,
        expected_job_type: &str,
        payload_key: &str,
        expected_resource_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let Some(job) = background_job::Entity::find_by_id(job_id)
            .lock(LockType::Update)
            .one(db)
            .await
            .map_err(database_error)?
        else {
            return Ok(false);
        };
        if job.job_type != expected_job_type
            || linked_resource_id(&job, payload_key) != Some(expected_resource_id)
        {
            return Ok(false);
        }
        if job.status == background_job::Model::STATUS_PENDING
            || (job.status == background_job::Model::STATUS_RUNNING
                && job.lease_until.is_some_and(|lease_until| lease_until > now))
        {
            return Ok(true);
        }
        let mut active: background_job::ActiveModel = job.into();
        active.status = Set(background_job::Model::STATUS_PENDING.to_owned());
        active.attempts = Set(0);
        active.available_at = Set(now);
        active.lease_owner = Set(None);
        active.lease_until = Set(None);
        active.updated_at = Set(now);
        active.completed_at = Set(None);
        active.update(db).await.map_err(database_error)?;
        Ok(true)
    }

    /// 回收崩溃 Worker 遗留的过期租约。
    ///
    /// 任务被领取时即消耗一次尝试；最后一次已过期的租约直接进入 `dead`，其余任务
    /// 回到 `pending` 并立即可再次领取。先处理死信再重入队，避免耗尽任务被错误复活。
    pub(crate) async fn recover_expired_leases(
        &self,
        db: &DatabaseConnection,
        now: DateTime<Utc>,
        tenant_scope: &ExecutionTenantFilter,
    ) -> AppResult<ExpiredLeaseRecovery> {
        let transaction = db.begin().await.map_err(database_error)?;
        let mut query = background_job::Entity::find()
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseUntil.lte(now))
            .order_by_asc(background_job::Column::LeaseUntil)
            .order_by_asc(background_job::Column::Id);
        if let Some(condition) = tenant_scope.condition(background_job::Column::TenantId) {
            query = query.filter(condition);
        }
        let expired = query
            .lock_with_behavior(
                LockType::Update,
                sea_orm::sea_query::LockBehavior::SkipLocked,
            )
            .limit(500)
            .all(&transaction)
            .await
            .map_err(database_error)?;
        let mut recovery = ExpiredLeaseRecovery::default();
        for job in expired {
            let dead = job.attempts >= job.max_attempts;
            Self::sync_linked_job_state(
                &transaction,
                &job,
                if dead {
                    LinkedJobDisposition::Dead
                } else {
                    LinkedJobDisposition::Retried
                },
                Some(EXPIRED_LEASE_DEAD_ERROR),
                now,
            )
            .await?;

            let mut active: background_job::ActiveModel = job.into();
            active.status = Set(if dead {
                background_job::Model::STATUS_DEAD
            } else {
                background_job::Model::STATUS_PENDING
            }
            .to_owned());
            active.available_at = Set(now);
            active.lease_owner = Set(None);
            active.lease_until = Set(None);
            active.last_error = Set(dead.then(|| EXPIRED_LEASE_DEAD_ERROR.to_owned()));
            active.updated_at = Set(now);
            active.completed_at = Set(dead.then_some(now));
            active.update(&transaction).await.map_err(database_error)?;
            if dead {
                recovery.dead = recovery.dead.saturating_add(1);
            } else {
                recovery.requeued = recovery.requeued.saturating_add(1);
            }
        }
        transaction.commit().await.map_err(database_error)?;
        Ok(recovery)
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
        command: FailBackgroundJob<'_>,
    ) -> AppResult<JobFailureDisposition> {
        let FailBackgroundJob {
            job_id,
            worker_id,
            retry_at,
            error_message,
            force_dead,
            now,
        } = command;
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

        let dead = force_dead || job.attempts >= job.max_attempts;
        Self::sync_linked_job_state(
            &txn,
            &job,
            if dead {
                LinkedJobDisposition::Dead
            } else {
                LinkedJobDisposition::Retried
            },
            Some(error_message),
            now,
        )
        .await?;
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

    /// 将因短期资源占用而无法执行的任务延后，且不消耗一次业务尝试预算。
    pub async fn defer_retryable_conflict(
        &self,
        db: &DatabaseConnection,
        job_id: i64,
        worker_id: &str,
        available_at: DateTime<Utc>,
        error_message: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let transaction = db.begin().await.map_err(database_error)?;
        let Some(job) = Self::owned_running_query(job_id, worker_id)
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            rollback_quietly(transaction).await;
            return Ok(false);
        };
        if job.lease_until.is_none_or(|lease_until| lease_until <= now) {
            rollback_quietly(transaction).await;
            return Ok(false);
        }
        let linked_transitioned = Self::sync_linked_job_state(
            &transaction,
            &job,
            LinkedJobDisposition::Retried,
            Some(error_message),
            now,
        )
        .await?;
        if is_tenant_config_job(&job.job_type) && !linked_transitioned {
            rollback_quietly(transaction).await;
            return Ok(false);
        }
        let attempts = job.attempts.saturating_sub(1);
        let mut active: background_job::ActiveModel = job.into();
        active.status = Set(background_job::Model::STATUS_PENDING.to_owned());
        active.attempts = Set(attempts);
        active.available_at = Set(available_at);
        active.lease_owner = Set(None);
        active.lease_until = Set(None);
        active.last_error = Set(Some(truncate_error(error_message)));
        active.updated_at = Set(now);
        active.completed_at = Set(None);
        active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(true)
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
        let transaction = db.begin().await.map_err(database_error)?;
        let Some(job) = Self::owned_running_query(job_id, worker_id)
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            rollback_quietly(transaction).await;
            return Ok(false);
        };
        if job.lease_until.is_none_or(|lease_until| lease_until <= now) {
            rollback_quietly(transaction).await;
            return Ok(false);
        }
        Self::sync_linked_job_state(
            &transaction,
            &job,
            LinkedJobDisposition::Dead,
            Some(error_message),
            now,
        )
        .await?;
        let mut active: background_job::ActiveModel = job.into();
        active.status = Set(background_job::Model::STATUS_DEAD.to_owned());
        active.lease_owner = Set(None);
        active.lease_until = Set(None);
        active.last_error = Set(Some(truncate_error(error_message)));
        active.updated_at = Set(now);
        active.completed_at = Set(Some(now));
        active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(true)
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
        retry_requested_by: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let transaction = db.begin().await.map_err(database_error)?;
        let tenant_scope = if include_platform {
            sea_orm::Condition::any()
                .add(background_job::Column::TenantId.eq(tenant_id))
                .add(background_job::Column::TenantId.is_null())
        } else {
            sea_orm::Condition::all().add(background_job::Column::TenantId.eq(tenant_id))
        };
        let Some(job) = background_job::Entity::find_by_id(job_id)
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_DEAD))
            .filter(tenant_scope)
            .lock(LockType::Update)
            .one(&transaction)
            .await
            .map_err(database_error)?
        else {
            rollback_quietly(transaction).await;
            return Ok(false);
        };
        if is_tenant_config_job(&job.job_type)
            && !Self::is_tenant_config_job_owner(&transaction, &job, tenant_id, retry_requested_by)
                .await?
        {
            rollback_quietly(transaction).await;
            return Ok(false);
        }
        let linked_transitioned = Self::sync_linked_job_state(
            &transaction,
            &job,
            LinkedJobDisposition::ManuallyRetried,
            None,
            now,
        )
        .await?;
        if is_tenant_config_job(&job.job_type) && !linked_transitioned {
            rollback_quietly(transaction).await;
            return Ok(false);
        }
        let mut active: background_job::ActiveModel = job.into();
        active.status = Set(background_job::Model::STATUS_PENDING.to_owned());
        active.attempts = Set(0);
        active.available_at = Set(now);
        active.lease_owner = Set(None);
        active.lease_until = Set(None);
        active.updated_at = Set(now);
        active.completed_at = Set(None);
        active.update(&transaction).await.map_err(database_error)?;
        transaction.commit().await.map_err(database_error)?;
        Ok(true)
    }

    fn owned_running_query(
        job_id: i64,
        worker_id: &str,
    ) -> sea_orm::Select<background_job::Entity> {
        background_job::Entity::find_by_id(job_id)
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseOwner.eq(worker_id))
    }
}
