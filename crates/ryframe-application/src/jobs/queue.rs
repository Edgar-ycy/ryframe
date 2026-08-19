use std::{
    sync::{Arc, RwLock},
    time::Duration as StdDuration,
};

use chrono::{DateTime, Utc};
use ryframe_adapters::RedisClient;
use ryframe_adapters::repository::{PageResult, ValidatedPageQuery};
use ryframe_auth::RequestPrincipal;
use ryframe_db::{
    BackgroundJobFilter, BackgroundJobRepository, BackgroundJobStats, ControlDatabaseCluster,
    EnqueueBackgroundJob, EnqueueBackgroundJobResult, ExecutionTenantScope, background_job,
    tenant_config_bundle, tenant_config_transfer,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter};
use serde::Serialize;
use tokio::{sync::watch, task::JoinHandle};

use super::{
    metrics::JobMetricsObserver,
    wakeup::{QueueWakeup, WakeupQueue},
};
/// 后台任务分页列表的业务查询参数。
#[derive(Clone, Debug)]
pub struct BackgroundJobListParams {
    pub page: ValidatedPageQuery,
    pub schedule_id: Option<String>,
    pub job_type: Option<String>,
    pub status: Option<String>,
}

/// 面向管理端的后台任务安全视图。
///
/// 任务载荷可能包含业务敏感字段，因此监控列表不会返回 `payload`。
#[derive(Clone, Debug, Serialize)]
pub struct BackgroundJobVo {
    pub id: String,
    pub schedule_id: Option<String>,
    pub scheduled_for: Option<DateTime<Utc>>,
    pub max_runtime_seconds: Option<i32>,
    pub job_type: String,
    pub status: String,
    pub priority: i32,
    pub available_at: DateTime<Utc>,
    pub attempts: i32,
    pub max_attempts: i32,
    pub lease_owner: Option<String>,
    pub lease_until: Option<DateTime<Utc>>,
    pub dedupe_key: Option<String>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl From<background_job::Model> for BackgroundJobVo {
    fn from(job: background_job::Model) -> Self {
        let last_error = public_job_error(&job.job_type, job.last_error);
        Self {
            id: job.id.to_string(),
            schedule_id: job.schedule_id.map(|id| id.to_string()),
            scheduled_for: job.scheduled_for,
            max_runtime_seconds: job.max_runtime_seconds,
            job_type: job.job_type,
            status: job.status,
            priority: job.priority,
            available_at: job.available_at,
            attempts: job.attempts,
            max_attempts: job.max_attempts,
            lease_owner: job.lease_owner,
            lease_until: job.lease_until,
            dedupe_key: job.dedupe_key,
            last_error,
            created_at: job.created_at,
            updated_at: job.updated_at,
            completed_at: job.completed_at,
        }
    }
}

/// 当前租户的后台任务队列统计。
#[derive(Clone, Copy, Debug, Serialize)]
pub struct BackgroundJobQueueStats {
    pub total: u64,
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub dead: u64,
    pub ready: u64,
}

impl From<BackgroundJobStats> for BackgroundJobQueueStats {
    fn from(stats: BackgroundJobStats) -> Self {
        Self {
            total: stats.total,
            pending: stats.pending,
            running: stats.running,
            succeeded: stats.succeeded,
            dead: stats.dead,
            ready: stats.ready,
        }
    }
}

/// 持久化任务的业务层入口。
#[derive(Clone)]
pub struct JobQueue {
    database: ControlDatabaseCluster,
    repository: Arc<BackgroundJobRepository>,
    metrics_observer: Arc<RwLock<Option<Arc<dyn JobMetricsObserver>>>>,
    wakeup: Arc<QueueWakeup>,
}

impl JobQueue {
    /// 使用主库构造任务队列。所有领取、状态迁移和入队都必须走主库。
    pub fn new(database: ControlDatabaseCluster) -> Self {
        let metrics_observer = Arc::new(RwLock::new(None));
        Self {
            database,
            repository: Arc::new(BackgroundJobRepository),
            wakeup: Arc::new(QueueWakeup::new(None, metrics_observer.clone())),
            metrics_observer,
        }
    }

    /// 配置可选 Redis 唤醒提示；未配置 Redis 时仍保留本进程本地唤醒。
    pub fn with_wakeup_redis(mut self, redis: Option<RedisClient>) -> Self {
        self.wakeup = Arc::new(QueueWakeup::new(redis, self.metrics_observer.clone()));
        self
    }

    /// 在业务 control 事务内原地复活已关联任务，不创建并发副本。
    pub async fn reactivate_linked_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        job_id: i64,
        expected_job_type: &str,
        payload_key: &str,
        expected_resource_id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        self.repository
            .reactivate_linked_in_txn(
                transaction,
                job_id,
                expected_job_type,
                payload_key,
                expected_resource_id,
                now,
            )
            .await
    }

    /// 安装应用层提供的任务指标观察者。
    pub fn with_metrics_observer(self, observer: Arc<dyn JobMetricsObserver>) -> Self {
        self.set_metrics_observer(observer);
        self
    }

    /// 在运行时安装或替换应用层提供的任务指标观察者。
    pub fn set_metrics_observer(&self, observer: Arc<dyn JobMetricsObserver>) {
        let mut stored = self
            .metrics_observer
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *stored = Some(observer);
    }

    /// 汇报所有已注册任务类型的低基数队列指标。
    pub async fn report_metrics_for_types(
        &self,
        job_types: &[String],
        tenant_scope: &ExecutionTenantScope,
    ) -> AppResult<()> {
        let Some(observer) = self.metrics_observer() else {
            return Ok(());
        };
        for stats in self
            .repository
            .stats_for_types(self.primary(), job_types, tenant_scope)
            .await?
        {
            observer.set_queue_depth(&stats.job_type, "pending", stats.pending);
            observer.set_queue_depth(&stats.job_type, "running", stats.running);
            observer.set_queue_depth(&stats.job_type, "dead", stats.dead);
            observer.set_queue_depth(&stats.job_type, "ready", stats.ready);
            observer
                .set_oldest_ready_age(&stats.job_type, stats.oldest_ready_age.unwrap_or_default());
        }
        Ok(())
    }

    /// 获取数据库当前 UTC 时间，避免多个 Worker 依赖不同机器时钟。
    pub async fn database_now(&self) -> AppResult<DateTime<Utc>> {
        self.repository
            .database_utc_now(self.database.write())
            .await
    }

    /// 回收崩溃 Worker 遗留的过期任务租约。
    pub async fn recover_expired_leases(
        &self,
        tenant_scope: &ExecutionTenantScope,
    ) -> AppResult<()> {
        loop {
            let now = self.database_now().await?;
            let recovered = self
                .repository
                .recover_expired_leases(self.primary(), now, tenant_scope)
                .await?;
            if recovered.requeued.saturating_add(recovered.dead) < 500 {
                break;
            }
        }
        Ok(())
    }

    /// 写入一条任务。调用方不需要自行提供时间戳，时间统一由数据库确定。
    pub async fn enqueue(
        &self,
        command: EnqueueBackgroundJob,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        let now = self.database_now().await?;
        let result = self
            .repository
            .enqueue(self.database.write(), command, now)
            .await?;
        self.notify_background_jobs().await;
        Ok(result)
    }

    /// 在既有业务事务中写入任务，保证业务数据和任务记录一起提交或回滚。
    ///
    /// 调用方在提交成功后应调用 notify_background_jobs；该提示只缩短等待时间，
    /// 任务可靠性仍由数据库轮询和租约机制保证。
    pub async fn enqueue_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        command: EnqueueBackgroundJob,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        let now = self.repository.database_utc_now(transaction).await?;
        self.repository
            .enqueue_in_transaction(transaction, command, now)
            .await
    }

    /// 仅供持有 control 业务资源的 watchdog 核对权威关联任务。
    pub async fn linked_job(&self, job_id: i64) -> AppResult<Option<background_job::Model>> {
        self.repository.find_by_id(self.primary(), job_id).await
    }

    /// 查询当前租户的后台任务；任务类型和状态均为精确匹配。
    pub async fn list_for_tenant(
        &self,
        actor: &ActorContext,
        params: BackgroundJobListParams,
    ) -> AppResult<PageResult<BackgroundJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let include_platform = tenant_id == "system";
        let job_type = normalize_job_type_filter(params.job_type)?;
        let status = normalize_job_status_filter(params.status)?;
        let schedule_id = normalize_schedule_id_filter(params.schedule_id)?;
        let page = self
            .repository
            .list(
                self.primary(),
                BackgroundJobFilter {
                    tenant_id: Some(tenant_id),
                    include_platform,
                    schedule_id,
                    job_type: job_type.as_deref(),
                    status: status.as_deref(),
                },
                &params.page,
            )
            .await?;

        Ok(PageResult {
            records: page
                .records
                .into_iter()
                .map(BackgroundJobVo::from)
                .collect(),
            total: page.total,
            page: page.page,
            page_size: page.page_size,
        })
    }

    /// 统计当前租户的任务状态，使用数据库时钟界定可执行任务。
    pub async fn stats_for_tenant(
        &self,
        actor: &ActorContext,
    ) -> AppResult<BackgroundJobQueueStats> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let include_platform = tenant_id == "system";
        let now = self.database_now().await?;
        self.repository
            .stats_filtered(
                self.primary(),
                BackgroundJobFilter {
                    tenant_id: Some(tenant_id),
                    include_platform,
                    ..Default::default()
                },
                now,
            )
            .await
            .map(BackgroundJobQueueStats::from)
    }

    /// 重新投递当前租户的一条死信任务，并返回更新后的安全视图。
    pub async fn retry_dead_for_tenant(
        &self,
        principal: &RequestPrincipal,
        job_id: i64,
    ) -> AppResult<BackgroundJobVo> {
        if job_id <= 0 {
            return Err(AppError::Validation("后台任务 ID 必须是正整数".into()));
        }
        let tenant_id = crate::validated_tenant_id(principal)?;
        let include_platform = tenant_id == "system";
        let existing = self
            .repository
            .find_by_id_for_tenant(self.primary(), tenant_id, include_platform, job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("后台任务不存在或不属于当前租户".into()))?;
        if existing.status != background_job::Model::STATUS_DEAD {
            return Err(AppError::Conflict("仅允许重新投递死信任务".into()));
        }
        if let Some(required_permission) = manual_retry_permission(&existing.job_type)
            && !principal.is_super_admin
            && !ryframe_auth::rbac::has_permission(&principal.permissions, required_permission)
        {
            return Err(AppError::Authorization(format!(
                "重新投递该业务任务还需要权限：{required_permission}"
            )));
        }
        self.ensure_tenant_config_retry_owner(principal, &existing)
            .await?;

        let now = self.database_now().await?;
        let retried = self
            .repository
            .retry_dead(
                self.primary(),
                tenant_id,
                include_platform,
                job_id,
                principal.user_id,
                now,
            )
            .await?;
        if !retried {
            return Err(AppError::Conflict(
                "后台任务状态已变化，请刷新后重试".into(),
            ));
        }
        self.notify_background_jobs().await;

        self.repository
            .find_by_id_for_tenant(self.primary(), tenant_id, include_platform, job_id)
            .await?
            .map(BackgroundJobVo::from)
            .ok_or_else(|| AppError::Internal("后台任务重试后无法读取".into()))
    }

    async fn ensure_tenant_config_retry_owner(
        &self,
        principal: &RequestPrincipal,
        job: &background_job::Model,
    ) -> AppResult<()> {
        let tenant_id = crate::validated_tenant_id(principal)?;
        if manual_retry_permission(&job.job_type).is_some()
            && job.tenant_id.as_deref() != Some(tenant_id)
        {
            return Err(AppError::NotFound(
                "后台任务关联的配置资源不存在或不可访问".into(),
            ));
        }
        let owner_id = match job.job_type.as_str() {
            "system.tenant_config.export" => tenant_config_bundle::Entity::find()
                .filter(tenant_config_bundle::Column::TenantId.eq(tenant_id))
                .filter(tenant_config_bundle::Column::BackgroundJobId.eq(job.id))
                .one(self.primary())
                .await
                .map_err(database_error)?
                .map(|bundle| bundle.created_by),
            "system.tenant_config.preview" => {
                transfer_job_owner(self.primary(), tenant_id, job.id, TransferJobKind::Preview)
                    .await?
            }
            "system.tenant_config.apply" => {
                transfer_job_owner(self.primary(), tenant_id, job.id, TransferJobKind::Apply)
                    .await?
            }
            "system.tenant_config.rollback" => {
                transfer_job_owner(self.primary(), tenant_id, job.id, TransferJobKind::Rollback)
                    .await?
            }
            _ => return Ok(()),
        }
        .ok_or_else(|| AppError::NotFound("后台任务关联的配置资源不存在".into()))?;
        if owner_id != principal.user_id {
            return Err(AppError::Authorization(
                "仅允许原配置任务申请人重新投递该任务".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn repository(&self) -> &BackgroundJobRepository {
        &self.repository
    }

    pub(super) fn has_metrics_observer(&self) -> bool {
        self.metrics_observer().is_some()
    }

    pub(super) fn observe_job_duration(
        &self,
        job_type: &str,
        result: &'static str,
        duration: StdDuration,
    ) {
        if let Some(observer) = self.metrics_observer() {
            observer.observe_duration(job_type, result, duration);
        }
    }

    pub(super) fn metrics_observer(&self) -> Option<Arc<dyn JobMetricsObserver>> {
        self.metrics_observer
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn primary(&self) -> &sea_orm::DatabaseConnection {
        self.database.write()
    }

    /// 在任务已成功提交后向本地与可选 Redis 等待者发送提示。
    pub async fn notify_background_jobs(&self) {
        self.wakeup.notify(WakeupQueue::BackgroundJob).await;
    }

    /// 在 Outbox 记录已成功提交后向本地与可选 Redis 等待者发送提示。
    pub async fn notify_outbox(&self) {
        self.wakeup.notify(WakeupQueue::Outbox).await;
    }

    pub(super) fn subscribe_background_job_wakeups(&self) -> watch::Receiver<u64> {
        self.wakeup.subscribe(WakeupQueue::BackgroundJob)
    }

    pub(super) fn subscribe_outbox_wakeups(&self) -> watch::Receiver<u64> {
        self.wakeup.subscribe(WakeupQueue::Outbox)
    }

    pub(super) fn spawn_wakeup_listener(
        &self,
        shutdown: watch::Receiver<bool>,
    ) -> Option<JoinHandle<()>> {
        self.wakeup.spawn_redis_listener(shutdown)
    }

    pub(super) fn record_claim_attempt(&self, queue: &'static str, result: &'static str) {
        if let Some(observer) = self.metrics_observer() {
            observer.record_claim_attempt(queue, result);
        }
    }
}

fn manual_retry_permission(job_type: &str) -> Option<&'static str> {
    match job_type {
        "system.tenant_config.export" => Some("system:config-package:export"),
        "system.tenant_config.preview" => Some("system:config-transfer:preview"),
        "system.tenant_config.apply" => Some("system:config-transfer:apply"),
        "system.tenant_config.rollback" => Some("system:config-transfer:rollback"),
        _ => None,
    }
}

fn public_job_error(job_type: &str, error: Option<String>) -> Option<String> {
    error.map(|error| match job_type {
        "system.tenant_config.export" => "配置包生成失败，请稍后重试或联系管理员".to_owned(),
        "system.tenant_config.preview" => "配置预览失败，请稍后重试或联系管理员".to_owned(),
        "system.tenant_config.apply" => "配置应用失败，请稍后重试或联系管理员".to_owned(),
        "system.tenant_config.rollback" => "配置回滚失败，请稍后重试或联系管理员".to_owned(),
        _ => error,
    })
}

#[derive(Clone, Copy)]
enum TransferJobKind {
    Preview,
    Apply,
    Rollback,
}

async fn transfer_job_owner(
    db: &sea_orm::DatabaseConnection,
    tenant_id: &str,
    job_id: i64,
    kind: TransferJobKind,
) -> AppResult<Option<i64>> {
    let query = tenant_config_transfer::Entity::find()
        .filter(tenant_config_transfer::Column::TenantId.eq(tenant_id));
    let query = match kind {
        TransferJobKind::Preview => {
            query.filter(tenant_config_transfer::Column::PreviewBackgroundJobId.eq(job_id))
        }
        TransferJobKind::Apply => {
            query.filter(tenant_config_transfer::Column::ApplyBackgroundJobId.eq(job_id))
        }
        TransferJobKind::Rollback => {
            query.filter(tenant_config_transfer::Column::RollbackBackgroundJobId.eq(job_id))
        }
    };
    query
        .one(db)
        .await
        .map_err(database_error)
        .map(|transfer| transfer.map(|transfer| transfer.requested_by))
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

fn normalize_job_type_filter(value: Option<String>) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() || value.len() > 96 {
        return Err(AppError::Validation(
            "任务类型长度必须在 1 到 96 个字节之间".into(),
        ));
    }
    Ok(Some(value.to_owned()))
}

fn normalize_schedule_id_filter(value: Option<String>) -> AppResult<Option<i64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    let schedule_id = value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation("来源计划 ID 必须是正整数".into()))?;
    Ok(Some(schedule_id))
}

fn normalize_job_status_filter(value: Option<String>) -> AppResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if !matches!(
        value,
        background_job::Model::STATUS_PENDING
            | background_job::Model::STATUS_RUNNING
            | background_job::Model::STATUS_SUCCEEDED
            | background_job::Model::STATUS_DEAD
    ) {
        return Err(AppError::Validation(
            "任务状态只能是 pending、running、succeeded 或 dead".into(),
        ));
    }
    Ok(Some(value.to_owned()))
}
