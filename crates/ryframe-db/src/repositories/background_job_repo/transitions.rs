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
    repositories::DataRetentionRepository,
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

fn is_tenant_config_job(job_type: &str) -> bool {
    matches!(
        job_type,
        TENANT_CONFIG_EXPORT_JOB_TYPE
            | TENANT_CONFIG_PREVIEW_JOB_TYPE
            | TENANT_CONFIG_APPLY_JOB_TYPE
            | TENANT_CONFIG_ROLLBACK_JOB_TYPE
    )
}

fn linked_resource_id(job: &background_job::Model, key: &str) -> Option<i64> {
    match job.payload.get(key)? {
        serde_json::Value::String(value) => value.parse().ok().filter(|id| *id > 0),
        serde_json::Value::Number(value) => value.as_i64().filter(|id| *id > 0),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum LinkedJobDisposition {
    Retried,
    Dead,
    ManuallyRetried,
}

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
        let transaction = db.begin().await.map_err(database_error)?;
        let expired = background_job::Entity::find()
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseUntil.lte(now))
            .order_by_asc(background_job::Column::LeaseUntil)
            .order_by_asc(background_job::Column::Id)
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

    async fn is_tenant_config_job_owner<C>(
        db: &C,
        job: &background_job::Model,
        tenant_id: &str,
        retry_requested_by: i64,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        if job.tenant_id.as_deref() != Some(tenant_id) {
            return Ok(false);
        }
        match job.job_type.as_str() {
            TENANT_CONFIG_EXPORT_JOB_TYPE => {
                let Some(bundle_id) = linked_resource_id(job, "bundle_id") else {
                    return Ok(false);
                };
                tenant_config_bundle::Entity::find_by_id(bundle_id)
                    .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                    .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job.id))
                    .filter(tenant_config_bundle::Column::CreatedBy.eq(retry_requested_by))
                    .lock(LockType::Update)
                    .one(db)
                    .await
                    .map(|bundle| bundle.is_some())
                    .map_err(database_error)
            }
            TENANT_CONFIG_PREVIEW_JOB_TYPE
            | TENANT_CONFIG_APPLY_JOB_TYPE
            | TENANT_CONFIG_ROLLBACK_JOB_TYPE => {
                let Some(transfer_id) = linked_resource_id(job, "transfer_id") else {
                    return Ok(false);
                };
                let query = tenant_config_transfer::Entity::find_by_id(transfer_id)
                    .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
                    .filter(tenant_config_transfer::Column::RequestedBy.eq(retry_requested_by));
                let query = match job.job_type.as_str() {
                    TENANT_CONFIG_PREVIEW_JOB_TYPE => query
                        .filter(tenant_config_transfer::Column::PreviewBackgroundJobId.eq(job.id)),
                    TENANT_CONFIG_APPLY_JOB_TYPE => query
                        .filter(tenant_config_transfer::Column::ApplyBackgroundJobId.eq(job.id)),
                    TENANT_CONFIG_ROLLBACK_JOB_TYPE => query
                        .filter(tenant_config_transfer::Column::RollbackBackgroundJobId.eq(job.id)),
                    _ => unreachable!("配置迁移任务类型已经过匹配"),
                };
                query
                    .lock(LockType::Update)
                    .one(db)
                    .await
                    .map(|transfer| transfer.is_some())
                    .map_err(database_error)
            }
            _ => Ok(true),
        }
    }

    fn owned_running_query(
        job_id: i64,
        worker_id: &str,
    ) -> sea_orm::Select<background_job::Entity> {
        background_job::Entity::find_by_id(job_id)
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseOwner.eq(worker_id))
    }

    async fn sync_linked_job_state<C>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error_message: Option<&str>,
        now: DateTime<Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let error = error_message.map(truncate_error);
        let mut linked_transitioned = true;
        match job.job_type.as_str() {
            USER_IMPORT_JOB_TYPE => {
                if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    let result = user_import_job::Entity::update_many()
                        .col_expr(
                            user_import_job::Column::LastError,
                            Expr::value(Option::<String>::None),
                        )
                        .col_expr(user_import_job::Column::UpdatedAt, Expr::value(now))
                        .filter(user_import_job::Column::BackgroundJobId.eq(job.id))
                        .filter(user_import_job::Column::Status.is_in([
                            user_import_job::Model::STATUS_SUCCEEDED,
                            user_import_job::Model::STATUS_PARTIAL,
                        ]))
                        .exec(db)
                        .await
                        .map_err(database_error)?;
                    if result.rows_affected > 0 {
                        return Ok(true);
                    }
                }
                let (status, completed_at, statuses) = match disposition {
                    LinkedJobDisposition::Retried => (
                        user_import_job::Model::STATUS_PENDING,
                        None,
                        vec![
                            user_import_job::Model::STATUS_PENDING,
                            user_import_job::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::Dead => (
                        user_import_job::Model::STATUS_FAILED,
                        Some(now),
                        vec![
                            user_import_job::Model::STATUS_PENDING,
                            user_import_job::Model::STATUS_RUNNING,
                            user_import_job::Model::STATUS_FAILED,
                        ],
                    ),
                    LinkedJobDisposition::ManuallyRetried => (
                        user_import_job::Model::STATUS_PENDING,
                        None,
                        vec![user_import_job::Model::STATUS_FAILED],
                    ),
                };
                let mut update = user_import_job::Entity::update_many()
                    .col_expr(user_import_job::Column::Status, Expr::value(status))
                    .col_expr(
                        user_import_job::Column::CompletedAt,
                        Expr::value(completed_at),
                    )
                    .col_expr(user_import_job::Column::UpdatedAt, Expr::value(now))
                    .filter(user_import_job::Column::BackgroundJobId.eq(job.id))
                    .filter(user_import_job::Column::Status.is_in(statuses));
                if let Some(error) = error {
                    update = update
                        .col_expr(user_import_job::Column::LastError, Expr::value(Some(error)));
                } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    update = update.col_expr(
                        user_import_job::Column::LastError,
                        Expr::value(Option::<String>::None),
                    );
                }
                update.exec(db).await.map_err(database_error)?;
            }
            EXPORT_JOB_TYPE => {
                let (status, completed_at, statuses) = match disposition {
                    LinkedJobDisposition::Retried => (
                        export_job::Model::STATUS_QUEUED,
                        None,
                        vec![
                            export_job::Model::STATUS_QUEUED,
                            export_job::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::Dead => (
                        export_job::Model::STATUS_FAILED,
                        Some(now),
                        vec![
                            export_job::Model::STATUS_QUEUED,
                            export_job::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::ManuallyRetried => (
                        export_job::Model::STATUS_QUEUED,
                        None,
                        vec![export_job::Model::STATUS_FAILED],
                    ),
                };
                let mut update = export_job::Entity::update_many()
                    .col_expr(export_job::Column::Status, Expr::value(status))
                    .col_expr(export_job::Column::CompletedAt, Expr::value(completed_at))
                    .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
                    .filter(export_job::Column::BackgroundJobId.eq(job.id))
                    .filter(export_job::Column::Status.is_in(statuses));
                if let Some(error) = error {
                    update =
                        update.col_expr(export_job::Column::ErrorMessage, Expr::value(Some(error)));
                } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    update = update.col_expr(
                        export_job::Column::ErrorMessage,
                        Expr::value(Option::<String>::None),
                    );
                }
                update.exec(db).await.map_err(database_error)?;
            }
            DATA_RETENTION_JOB_TYPE => {
                Self::ensure_retention_run(db, job, now).await?;
                let (status, completed_at, statuses) = match disposition {
                    LinkedJobDisposition::Retried => (
                        data_retention_run::Model::STATUS_PENDING,
                        None,
                        vec![
                            data_retention_run::Model::STATUS_PENDING,
                            data_retention_run::Model::STATUS_RUNNING,
                            data_retention_run::Model::STATUS_FAILED,
                        ],
                    ),
                    LinkedJobDisposition::Dead => (
                        data_retention_run::Model::STATUS_FAILED,
                        Some(now),
                        vec![
                            data_retention_run::Model::STATUS_PENDING,
                            data_retention_run::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::ManuallyRetried => (
                        data_retention_run::Model::STATUS_PENDING,
                        None,
                        vec![data_retention_run::Model::STATUS_FAILED],
                    ),
                };
                let mut update = data_retention_run::Entity::update_many()
                    .col_expr(data_retention_run::Column::Status, Expr::value(status))
                    .col_expr(
                        data_retention_run::Column::CompletedAt,
                        Expr::value(completed_at),
                    )
                    .col_expr(data_retention_run::Column::UpdatedAt, Expr::value(now))
                    .filter(data_retention_run::Column::BackgroundJobId.eq(job.id))
                    .filter(data_retention_run::Column::Status.is_in(statuses));
                if let Some(error) = error {
                    update = update.col_expr(
                        data_retention_run::Column::ErrorSummary,
                        Expr::value(Some(error)),
                    );
                } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    update = update.col_expr(
                        data_retention_run::Column::ErrorSummary,
                        Expr::value(Option::<String>::None),
                    );
                }
                update.exec(db).await.map_err(database_error)?;
            }
            TENANT_CONFIG_EXPORT_JOB_TYPE => {
                let (status, statuses) = match disposition {
                    LinkedJobDisposition::Retried => (
                        tenant_config_bundle::Model::STATUS_PENDING,
                        vec![
                            tenant_config_bundle::Model::STATUS_PENDING,
                            tenant_config_bundle::Model::STATUS_RUNNING,
                        ],
                    ),
                    LinkedJobDisposition::Dead => (
                        tenant_config_bundle::Model::STATUS_FAILED,
                        vec![
                            tenant_config_bundle::Model::STATUS_PENDING,
                            tenant_config_bundle::Model::STATUS_RUNNING,
                            tenant_config_bundle::Model::STATUS_FAILED,
                        ],
                    ),
                    LinkedJobDisposition::ManuallyRetried => (
                        tenant_config_bundle::Model::STATUS_PENDING,
                        vec![tenant_config_bundle::Model::STATUS_FAILED],
                    ),
                };
                let mut update = tenant_config_bundle::Entity::update_many()
                    .col_expr(tenant_config_bundle::Column::Status, Expr::value(status))
                    .col_expr(tenant_config_bundle::Column::UpdatedAt, Expr::value(now))
                    .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job.id))
                    .filter(tenant_config_bundle::Column::Status.is_in(statuses));
                if error.is_some() {
                    update = update.col_expr(
                        tenant_config_bundle::Column::ErrorSummary,
                        Expr::value(Some(TENANT_CONFIG_EXPORT_SAFE_ERROR.to_owned())),
                    );
                } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
                    update = update.col_expr(
                        tenant_config_bundle::Column::ErrorSummary,
                        Expr::value(Option::<String>::None),
                    );
                }
                linked_transitioned = update
                    .exec(db)
                    .await
                    .map(|result| result.rows_affected == 1)
                    .map_err(database_error)?;
            }
            TENANT_CONFIG_PREVIEW_JOB_TYPE => {
                linked_transitioned = Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Preview,
                )
                .await?;
            }
            TENANT_CONFIG_APPLY_JOB_TYPE => {
                linked_transitioned = Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Apply,
                )
                .await?;
            }
            TENANT_CONFIG_ROLLBACK_JOB_TYPE => {
                linked_transitioned = Self::sync_config_transfer_state(
                    db,
                    job,
                    disposition,
                    error,
                    now,
                    ConfigTransferJobKind::Rollback,
                )
                .await?;
            }
            _ => {}
        }
        Ok(linked_transitioned)
    }

    async fn sync_config_transfer_state<C>(
        db: &C,
        job: &background_job::Model,
        disposition: LinkedJobDisposition,
        error: Option<String>,
        now: DateTime<Utc>,
        kind: ConfigTransferJobKind,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let (status, statuses) = match (kind, disposition) {
            (ConfigTransferJobKind::Preview, LinkedJobDisposition::Retried) => (
                tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
                vec![
                    tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
                    tenant_config_transfer::Model::STATUS_PREVIEWING,
                ],
            ),
            (ConfigTransferJobKind::Preview, LinkedJobDisposition::Dead) => (
                tenant_config_transfer::Model::STATUS_FAILED,
                vec![
                    tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
                    tenant_config_transfer::Model::STATUS_PREVIEWING,
                    tenant_config_transfer::Model::STATUS_FAILED,
                ],
            ),
            (ConfigTransferJobKind::Preview, LinkedJobDisposition::ManuallyRetried) => (
                tenant_config_transfer::Model::STATUS_PREVIEW_PENDING,
                vec![tenant_config_transfer::Model::STATUS_FAILED],
            ),
            (ConfigTransferJobKind::Apply, LinkedJobDisposition::Retried) => (
                tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                vec![
                    tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                    tenant_config_transfer::Model::STATUS_APPLYING,
                ],
            ),
            (ConfigTransferJobKind::Apply, LinkedJobDisposition::Dead) => (
                tenant_config_transfer::Model::STATUS_FAILED,
                vec![
                    tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                    tenant_config_transfer::Model::STATUS_APPLYING,
                    tenant_config_transfer::Model::STATUS_FAILED,
                ],
            ),
            (ConfigTransferJobKind::Apply, LinkedJobDisposition::ManuallyRetried) => (
                tenant_config_transfer::Model::STATUS_APPLY_PENDING,
                vec![tenant_config_transfer::Model::STATUS_FAILED],
            ),
            (ConfigTransferJobKind::Rollback, LinkedJobDisposition::Retried) => (
                tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                vec![
                    tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                    tenant_config_transfer::Model::STATUS_ROLLING_BACK,
                ],
            ),
            (ConfigTransferJobKind::Rollback, LinkedJobDisposition::Dead) => (
                tenant_config_transfer::Model::STATUS_FAILED,
                vec![
                    tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                    tenant_config_transfer::Model::STATUS_ROLLING_BACK,
                    tenant_config_transfer::Model::STATUS_FAILED,
                ],
            ),
            (ConfigTransferJobKind::Rollback, LinkedJobDisposition::ManuallyRetried) => (
                tenant_config_transfer::Model::STATUS_ROLLBACK_PENDING,
                vec![tenant_config_transfer::Model::STATUS_FAILED],
            ),
        };
        let Some(transfer_id) = linked_resource_id(job, "transfer_id") else {
            return Ok(false);
        };
        let Some(tenant_id) = job.tenant_id.as_deref() else {
            return Ok(false);
        };
        let mut update = tenant_config_transfer::Entity::update_many()
            .col_expr(tenant_config_transfer::Column::Status, Expr::value(status))
            .col_expr(tenant_config_transfer::Column::UpdatedAt, Expr::value(now))
            .filter(tenant_config_transfer::Column::Id.eq(transfer_id))
            .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id))
            .filter(tenant_config_transfer::Column::Status.is_in(statuses));
        update = match kind {
            ConfigTransferJobKind::Preview => {
                update.filter(tenant_config_transfer::Column::PreviewBackgroundJobId.eq(job.id))
            }
            ConfigTransferJobKind::Apply => {
                update.filter(tenant_config_transfer::Column::ApplyBackgroundJobId.eq(job.id))
            }
            ConfigTransferJobKind::Rollback => {
                update.filter(tenant_config_transfer::Column::RollbackBackgroundJobId.eq(job.id))
            }
        };
        if error.is_some() {
            let safe_error = match kind {
                ConfigTransferJobKind::Preview => TENANT_CONFIG_PREVIEW_SAFE_ERROR,
                ConfigTransferJobKind::Apply => TENANT_CONFIG_APPLY_SAFE_ERROR,
                ConfigTransferJobKind::Rollback => TENANT_CONFIG_ROLLBACK_SAFE_ERROR,
            };
            update = update.col_expr(
                tenant_config_transfer::Column::ErrorSummary,
                Expr::value(Some(safe_error.to_owned())),
            );
        } else if matches!(disposition, LinkedJobDisposition::ManuallyRetried) {
            update = update.col_expr(
                tenant_config_transfer::Column::ErrorSummary,
                Expr::value(Option::<String>::None),
            );
        }
        update
            .exec(db)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(database_error)
    }

    async fn ensure_retention_run<C>(
        db: &C,
        job: &background_job::Model,
        now: DateTime<Utc>,
    ) -> AppResult<()>
    where
        C: ConnectionTrait,
    {
        if data_retention_run::Entity::find()
            .filter(data_retention_run::Column::BackgroundJobId.eq(job.id))
            .one(db)
            .await
            .map_err(database_error)?
            .is_some()
        {
            return Ok(());
        }
        let trigger_kind = job
            .payload
            .get("trigger_kind")
            .and_then(serde_json::Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    data_retention_run::Model::TRIGGER_MANUAL
                        | data_retention_run::Model::TRIGGER_SCHEDULED
                )
            })
            .unwrap_or(data_retention_run::Model::TRIGGER_SCHEDULED);
        let requested_by = job
            .payload
            .get("requested_by")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse::<i64>().ok());
        let run_id = job
            .payload
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(job.id);
        DataRetentionRepository
            .insert_run_if_missing(
                db,
                data_retention_run::Model {
                    id: run_id,
                    background_job_id: job.id,
                    trigger_kind: trigger_kind.to_owned(),
                    status: data_retention_run::Model::STATUS_PENDING.to_owned(),
                    policy_snapshot: serde_json::json!({}),
                    eligible_counts: serde_json::json!({}),
                    deleted_counts: serde_json::json!({}),
                    remaining_counts: serde_json::json!({}),
                    requested_by,
                    error_summary: None,
                    started_at: None,
                    completed_at: None,
                    created_at: now,
                    updated_at: now,
                },
            )
            .await?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum ConfigTransferJobKind {
    Preview,
    Apply,
    Rollback,
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
