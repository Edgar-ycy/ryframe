use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Utc};
use ryframe_kernel::{AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, ExprTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Statement, TransactionTrait,
    sea_query::{Expr, LockBehavior, LockType},
};
use serde_json::Value;

use crate::entities::background_job;

/// 租约到期且尝试次数耗尽时写入的安全诊断原因。
const EXPIRED_LEASE_DEAD_ERROR: &str = "任务租约已过期，处理结果未知";

/// 用于写入持久化异步任务的输入。
///
/// `dedupe_key` 按 `job_type` 隔离；提供该值后，首次调用创建任务，后续调用返回同一任务。
#[derive(Clone, Debug)]
pub struct EnqueueBackgroundJob {
    pub tenant_id: Option<String>,
    pub job_type: String,
    pub payload: Value,
    pub priority: i32,
    pub available_at: DateTime<Utc>,
    pub max_attempts: i32,
    pub dedupe_key: Option<String>,
    pub traceparent: Option<String>,
    pub tracestate: Option<String>,
}

/// 幂等入队操作的结果。
#[derive(Clone, Debug)]
pub struct EnqueueBackgroundJobResult {
    pub job: background_job::Model,
    /// `false` 表示其他请求已创建同一 `(job_type, dedupe_key)` 任务。
    pub inserted: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExpiredLeaseRecovery {
    pub requeued: u64,
    pub dead: u64,
}

#[derive(Clone, Debug, Default)]
pub struct BackgroundJobFilter<'a> {
    pub tenant_id: Option<&'a str>,
    pub job_type: Option<&'a str>,
    pub status: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackgroundJobStats {
    pub total: u64,
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub dead: u64,
    pub ready: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobFailureDisposition {
    /// 当前持有的租约已完成，任务会在指定时间重新变为可执行。
    Retried { available_at: DateTime<Utc> },
    /// 已耗尽最大领取次数。
    Dead,
    /// 其他 Worker 持有该租约，或该租约已过期并被重新领取。
    LeaseLost,
}

/// 仅适用于 MySQL 的持久化任务队列仓储。
pub struct BackgroundJobRepository;

impl BackgroundJobRepository {
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
            .ok_or_else(|| AppError::Database("database clock query returned no row".into()))?;
        let now: chrono::NaiveDateTime = row.try_get("", "db_now").map_err(database_error)?;
        Ok(DateTime::from_naive_utc_and_offset(now, Utc))
    }

    pub async fn enqueue(
        &self,
        db: &DatabaseConnection,
        command: EnqueueBackgroundJob,
        now: DateTime<Utc>,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        self.enqueue_on(db, command, now).await
    }

    /// 在调用方事务中入队，使业务数据与任务记录原子提交。
    pub async fn enqueue_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        command: EnqueueBackgroundJob,
        now: DateTime<Utc>,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        self.enqueue_on(transaction, command, now).await
    }

    async fn enqueue_on<C>(
        &self,
        db: &C,
        command: EnqueueBackgroundJob,
        now: DateTime<Utc>,
    ) -> AppResult<EnqueueBackgroundJobResult>
    where
        C: ConnectionTrait,
    {
        validate_enqueue_command(&command)?;
        let dedupe_identity = command
            .dedupe_key
            .as_ref()
            .map(|dedupe_key| (command.job_type.clone(), dedupe_key.clone()));

        if let Some((job_type, dedupe_key)) = dedupe_identity.as_ref()
            && let Some(existing) = self.find_by_dedupe_key_on(db, job_type, dedupe_key).await?
        {
            return Ok(EnqueueBackgroundJobResult {
                job: existing,
                inserted: false,
            });
        }

        let active = background_job::ActiveModel {
            id: Set(snowflake::try_next_snowflake_id()?),
            tenant_id: Set(command.tenant_id),
            job_type: Set(command.job_type),
            payload: Set(command.payload),
            status: Set(background_job::Model::STATUS_PENDING.to_owned()),
            priority: Set(command.priority),
            available_at: Set(command.available_at),
            attempts: Set(0),
            max_attempts: Set(command.max_attempts),
            lease_owner: Set(None),
            lease_until: Set(None),
            dedupe_key: Set(command.dedupe_key),
            traceparent: Set(command.traceparent),
            tracestate: Set(command.tracestate),
            last_error: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            completed_at: Set(None),
        };

        match active.insert(db).await {
            Ok(job) => Ok(EnqueueBackgroundJobResult {
                job,
                inserted: true,
            }),
            Err(error) if is_duplicate_key_error(&error) => {
                // 并发插入可能发生在预检查之后。MySQL 在报告 1062 前会等待获胜事务，
                // 因此后续查询能够读取到该记录。
                let (job_type, dedupe_key) = dedupe_identity.ok_or_else(|| {
                    AppError::Database(
                        "duplicate background job insert without a dedupe key".into(),
                    )
                })?;
                let existing = self
                    .find_by_dedupe_key_on(db, &job_type, &dedupe_key)
                    .await?
                    .ok_or_else(|| {
                        AppError::Database(
                            "duplicate background job key was reported but no row is readable"
                                .into(),
                        )
                    })?;
                Ok(EnqueueBackgroundJobResult {
                    job: existing,
                    inserted: false,
                })
            }
            Err(error) => Err(database_error(error)),
        }
    }

    pub async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<background_job::Model>> {
        background_job::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 查询当前租户可见的单个任务。
    pub async fn find_by_id_for_tenant(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<background_job::Model>> {
        background_job::Entity::find_by_id(id)
            .filter(background_job::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(database_error)
    }

    pub async fn find_by_dedupe_key(
        &self,
        db: &DatabaseConnection,
        job_type: &str,
        dedupe_key: &str,
    ) -> AppResult<Option<background_job::Model>> {
        self.find_by_dedupe_key_on(db, job_type, dedupe_key).await
    }

    async fn find_by_dedupe_key_on<C>(
        &self,
        db: &C,
        job_type: &str,
        dedupe_key: &str,
    ) -> AppResult<Option<background_job::Model>>
    where
        C: ConnectionTrait,
    {
        background_job::Entity::find()
            .filter(background_job::Column::JobType.eq(job_type))
            .filter(background_job::Column::DedupeKey.eq(dedupe_key))
            .one(db)
            .await
            .map_err(database_error)
    }

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

    /// 通过 `FOR UPDATE SKIP LOCKED` 领取一条可执行任务。
    ///
    /// 这是进入 `running` 的唯一状态迁移；行锁和 `attempts` 自增位于同一事务中。
    /// 因而进程在提交后崩溃时，任务会在独立的租约回收循环中再次投递。
    pub async fn claim_next(
        &self,
        db: &DatabaseConnection,
        worker_id: &str,
        lease_duration: Duration,
        now: DateTime<Utc>,
    ) -> AppResult<Option<background_job::Model>> {
        validate_lease(worker_id, lease_duration)?;
        let txn = db.begin().await.map_err(database_error)?;

        let Some(job) = Self::claimable_query(now)
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
        job_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let result = Self::retry_dead_query(tenant_id, job_id, now)
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    fn retry_dead_query(
        tenant_id: &str,
        job_id: i64,
        now: DateTime<Utc>,
    ) -> sea_orm::UpdateMany<background_job::Entity> {
        background_job::Entity::update_many()
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
            .filter(background_job::Column::TenantId.eq(tenant_id))
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_DEAD))
    }

    pub async fn list(
        &self,
        db: &DatabaseConnection,
        filter: BackgroundJobFilter<'_>,
        query: &ryframe_core::repository::ValidatedPageQuery,
    ) -> AppResult<ryframe_core::repository::PageResult<background_job::Model>> {
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
        let total = Self::filtered_query(filter.clone())
            .count(db)
            .await
            .map_err(database_error)?;
        let pending =
            Self::count_status(db, filter.clone(), background_job::Model::STATUS_PENDING).await?;
        let running =
            Self::count_status(db, filter.clone(), background_job::Model::STATUS_RUNNING).await?;
        let succeeded =
            Self::count_status(db, filter.clone(), background_job::Model::STATUS_SUCCEEDED).await?;
        let dead =
            Self::count_status(db, filter.clone(), background_job::Model::STATUS_DEAD).await?;
        let ready = Self::filtered_query(filter)
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_PENDING))
            .filter(background_job::Column::AvailableAt.lte(now))
            .filter(
                Expr::col(background_job::Column::Attempts)
                    .lt(Expr::col(background_job::Column::MaxAttempts)),
            )
            .count(db)
            .await
            .map_err(database_error)?;
        Ok(BackgroundJobStats {
            total,
            pending,
            running,
            succeeded,
            dead,
            ready,
        })
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

    fn claimable_query(now: DateTime<Utc>) -> sea_orm::Select<background_job::Entity> {
        background_job::Entity::find()
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_PENDING))
            .filter(background_job::Column::AvailableAt.lte(now))
            .filter(
                Expr::col(background_job::Column::Attempts)
                    .lt(Expr::col(background_job::Column::MaxAttempts)),
            )
            .order_by_desc(background_job::Column::Priority)
            .order_by_asc(background_job::Column::AvailableAt)
            .order_by_asc(background_job::Column::Id)
    }

    fn filtered_query(filter: BackgroundJobFilter<'_>) -> sea_orm::Select<background_job::Entity> {
        let mut select = background_job::Entity::find();
        if let Some(tenant_id) = filter.tenant_id {
            select = select.filter(background_job::Column::TenantId.eq(tenant_id));
        }
        if let Some(job_type) = filter.job_type {
            select = select.filter(background_job::Column::JobType.eq(job_type));
        }
        if let Some(status) = filter.status {
            select = select.filter(background_job::Column::Status.eq(status));
        }
        select
    }

    fn owned_running_query(
        job_id: i64,
        worker_id: &str,
    ) -> sea_orm::Select<background_job::Entity> {
        background_job::Entity::find_by_id(job_id)
            .filter(background_job::Column::Status.eq(background_job::Model::STATUS_RUNNING))
            .filter(background_job::Column::LeaseOwner.eq(worker_id))
    }

    async fn recover_expired_leases_on<C>(
        &self,
        db: &C,
        now: DateTime<Utc>,
    ) -> AppResult<ExpiredLeaseRecovery>
    where
        C: ConnectionTrait,
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

    async fn count_status(
        db: &DatabaseConnection,
        filter: BackgroundJobFilter<'_>,
        status: &str,
    ) -> AppResult<u64> {
        Self::filtered_query(filter)
            .filter(background_job::Column::Status.eq(status))
            .count(db)
            .await
            .map_err(database_error)
    }
}

fn validate_enqueue_command(command: &EnqueueBackgroundJob) -> AppResult<()> {
    if command.job_type.trim().is_empty() || command.job_type.len() > 96 {
        return Err(AppError::Validation(
            "background job type must contain 1 to 96 bytes".into(),
        ));
    }
    if command.max_attempts <= 0 {
        return Err(AppError::Validation(
            "background job max_attempts must be greater than zero".into(),
        ));
    }
    if command.max_attempts > 100 {
        return Err(AppError::Validation(
            "background job max_attempts must not exceed 100".into(),
        ));
    }
    if command
        .dedupe_key
        .as_deref()
        .is_some_and(|key| key.is_empty() || key.len() > 191)
    {
        return Err(AppError::Validation(
            "background job dedupe_key must contain 1 to 191 bytes when supplied".into(),
        ));
    }
    if command
        .traceparent
        .as_deref()
        .is_some_and(|value| value.len() > 255)
    {
        return Err(AppError::Validation(
            "background job traceparent must not exceed 255 bytes".into(),
        ));
    }
    if command
        .tracestate
        .as_deref()
        .is_some_and(|value| value.len() > 512)
    {
        return Err(AppError::Validation(
            "background job tracestate must not exceed 512 bytes".into(),
        ));
    }
    Ok(())
}

fn validate_lease(worker_id: &str, lease_duration: Duration) -> AppResult<()> {
    if worker_id.trim().is_empty() || worker_id.len() > 128 {
        return Err(AppError::Validation(
            "background job worker_id must contain 1 to 128 bytes".into(),
        ));
    }
    if lease_duration <= Duration::zero() {
        return Err(AppError::Validation(
            "background job lease duration must be positive".into(),
        ));
    }
    Ok(())
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

fn is_duplicate_key_error(error: &sea_orm::DbErr) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("duplicate") || text.contains("1062")
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

async fn rollback_quietly(transaction: DatabaseTransaction) {
    if let Err(error) = transaction.rollback().await {
        tracing::warn!(error = %error, "failed to rollback background job lease transaction");
    }
}
