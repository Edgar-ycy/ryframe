use std::{
    sync::{Arc, RwLock},
    time::Duration as StdDuration,
};

use chrono::{DateTime, Utc};
use ryframe_core::RedisClient;
use ryframe_core::repository::{PageResult, ValidatedPageQuery};
use ryframe_db::{
    BackgroundJobFilter, BackgroundJobRepository, BackgroundJobStats, DatabaseCluster,
    EnqueueBackgroundJob, EnqueueBackgroundJobResult, background_job,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::DatabaseTransaction;
use serde::Serialize;
use tokio::{sync::watch, task::JoinHandle, time};

use super::{
    metrics::JobMetricsObserver,
    wakeup::{QueueWakeup, WakeupQueue},
};
use crate::system::{EXPORT_CLEANUP_JOB_TYPE, MESSAGE_RETENTION_JOB_TYPE};

/// 后台任务分页列表的业务查询参数。
#[derive(Clone, Debug)]
pub struct BackgroundJobListParams {
    pub page: ValidatedPageQuery,
    pub schedule_id: Option<i64>,
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
            last_error: job.last_error,
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
    database: DatabaseCluster,
    repository: Arc<BackgroundJobRepository>,
    metrics_observer: Arc<RwLock<Option<Arc<dyn JobMetricsObserver>>>>,
    wakeup: Arc<QueueWakeup>,
}

impl JobQueue {
    /// 使用主库构造任务队列。所有领取、状态迁移和入队都必须走主库。
    pub fn new(database: DatabaseCluster) -> Self {
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
    pub async fn report_metrics_for_types(&self, job_types: &[String]) -> AppResult<()> {
        let Some(observer) = self.metrics_observer() else {
            return Ok(());
        };
        for stats in self
            .repository
            .stats_for_types(self.primary(), job_types)
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
    pub async fn recover_expired_leases(&self) -> AppResult<()> {
        let now = self.database_now().await?;
        self.repository
            .recover_expired_leases(self.primary(), now)
            .await?;
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

    /// 按 UTC 自然日幂等写入一次消息过期清理任务。
    pub async fn enqueue_message_retention(&self) -> AppResult<EnqueueBackgroundJobResult> {
        let now = self.database_now().await?;
        let day = now.format("%F").to_string();
        let trace_context = crate::trace_context::current_trace_context();
        let result = self
            .repository
            .enqueue(
                self.database.write(),
                EnqueueBackgroundJob {
                    tenant_id: None,
                    schedule_id: None,
                    scheduled_for: Some(now),
                    max_runtime_seconds: None,
                    job_type: MESSAGE_RETENTION_JOB_TYPE.to_owned(),
                    payload: serde_json::json!({ "run_date": day }),
                    priority: -10,
                    available_at: now,
                    max_attempts: 20,
                    dedupe_key: Some(format!("message:retention:{day}")),
                    traceparent: trace_context.traceparent,
                    tracestate: trace_context.tracestate,
                },
                now,
            )
            .await?;
        self.notify_background_jobs().await;
        Ok(result)
    }

    /// 按 UTC 自然日幂等写入一次导出结果清理任务。
    pub async fn enqueue_export_cleanup(&self) -> AppResult<EnqueueBackgroundJobResult> {
        let now = self.database_now().await?;
        let day = now.format("%F").to_string();
        let trace_context = crate::trace_context::current_trace_context();
        let result = self
            .repository
            .enqueue(
                self.database.write(),
                EnqueueBackgroundJob {
                    tenant_id: None,
                    schedule_id: None,
                    scheduled_for: Some(now),
                    max_runtime_seconds: None,
                    job_type: EXPORT_CLEANUP_JOB_TYPE.to_owned(),
                    payload: serde_json::json!({ "run_date": day }),
                    priority: -10,
                    available_at: now,
                    max_attempts: 20,
                    dedupe_key: Some(format!("export:cleanup:{day}")),
                    traceparent: trace_context.traceparent,
                    tracestate: trace_context.tracestate,
                },
                now,
            )
            .await?;
        self.notify_background_jobs().await;
        Ok(result)
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
        actor: &ActorContext,
        job_id: i64,
    ) -> AppResult<BackgroundJobVo> {
        if job_id <= 0 {
            return Err(AppError::Validation("后台任务 ID 必须是正整数".into()));
        }
        let tenant_id = crate::validated_tenant_id(actor)?;
        let include_platform = tenant_id == "system";
        let existing = self
            .repository
            .find_by_id_for_tenant(self.primary(), tenant_id, include_platform, job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("后台任务不存在或不属于当前租户".into()))?;
        if existing.status != background_job::Model::STATUS_DEAD {
            return Err(AppError::Conflict("仅允许重新投递死信任务".into()));
        }

        let now = self.database_now().await?;
        let retried = self
            .repository
            .retry_dead(self.primary(), tenant_id, include_platform, job_id, now)
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

    pub(super) fn record_schedule_scan(&self, result: &'static str) {
        if let Some(observer) = self.metrics_observer() {
            observer.record_schedule_scan(result);
        }
    }

    pub(super) fn record_schedule_trigger(&self, outcome: &'static str) {
        if let Some(observer) = self.metrics_observer() {
            observer.record_schedule_trigger(outcome);
        }
    }

    pub(super) fn observe_schedule_lag(&self, lag: StdDuration) {
        if let Some(observer) = self.metrics_observer() {
            observer.observe_schedule_lag(lag);
        }
    }
}

/// 在启动时及每个 UTC 自然日创建消息保留与导出结果清理任务。
///
/// 多个 API 或 Worker 实例可同时运行该调度器；数据库中的幂等键会确保每天只保留一条任务。
pub fn spawn_message_retention_scheduler(
    queue: Arc<JobQueue>,
    messaging_enabled: bool,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if messaging_enabled && let Err(error) = queue.enqueue_message_retention().await {
                tracing::warn!(%error, "无法写入每日消息过期清理任务");
            }
            if let Err(error) = queue.enqueue_export_cleanup().await {
                tracing::warn!(%error, "无法写入每日导出结果清理任务");
            }
            let now = queue.database_now().await.unwrap_or_else(|error| {
                tracing::warn!(%error, "无法读取数据库时间，按本机 UTC 时间安排下次消息清理");
                Utc::now()
            });
            let delay = duration_until_next_utc_day(now);
            tokio::select! {
                _ = time::sleep(delay) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    })
}

fn duration_until_next_utc_day(now: DateTime<Utc>) -> StdDuration {
    let Some(tomorrow) = now.date_naive().succ_opt() else {
        return StdDuration::from_secs(24 * 60 * 60);
    };
    let Some(next) = tomorrow.and_hms_opt(0, 0, 5) else {
        return StdDuration::from_secs(24 * 60 * 60);
    };
    (next.and_utc() - now)
        .to_std()
        .unwrap_or_else(|_| StdDuration::from_secs(60))
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

fn normalize_schedule_id_filter(value: Option<i64>) -> AppResult<Option<i64>> {
    if value.is_some_and(|id| id <= 0) {
        return Err(AppError::Validation("来源计划 ID 必须是正整数".into()));
    }
    Ok(value)
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
