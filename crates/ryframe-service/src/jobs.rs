use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
    time::Duration as StdDuration,
};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use ryframe_config::JobConfig;
use ryframe_core::RedisClient;
use ryframe_core::repository::{PageQuery, PageResult};
use ryframe_db::{
    BackgroundJobFilter, BackgroundJobRepository, BackgroundJobStats, DatabaseCluster,
    EnqueueBackgroundJob, EnqueueBackgroundJobResult, JobFailureDisposition, OutboxEventRepository,
    OutboxFailureDisposition, background_job, outbox_event,
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::{DatabaseTransaction, TransactionTrait};
use serde::{Deserialize, Serialize};
use tokio::{sync::watch, task::JoinHandle, time};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::system::{
    EXPORT_CLEANUP_JOB_TYPE, EXPORT_JOB_TYPE, ExportService, MESSAGE_DISPATCH_JOB_TYPE,
    MESSAGE_DISPATCH_REDIS_CHANNEL, MESSAGE_RETENTION_JOB_TYPE, MessageService, OperLogService,
    RecordOperLogCommand,
};

/// 操作日志任务的稳定类型标识。
pub const OPER_LOG_JOB_TYPE: &str = "system.oper_log.record";

/// 消息发布 Outbox 事件的稳定类型标识。
pub const MESSAGE_PUBLISHED_OUTBOX_EVENT_TYPE: &str = "system.message.published";

/// 后台任务监控的应用层观察者，避免业务层依赖具体指标实现。
pub trait JobMetricsObserver: Send + Sync {
    /// 更新一个已注册任务类型的队列状态计数。
    fn set_queue_depth(&self, job_type: &str, status: &'static str, depth: u64);

    /// 更新一个已注册任务类型中最早可执行任务的等待时长。
    fn set_oldest_ready_age(&self, job_type: &str, age: StdDuration);

    /// 记录一次已经被领取任务的处理时长。
    fn observe_duration(&self, job_type: &str, result: &'static str, duration: StdDuration);
}

type QueueDepthCallback = dyn Fn(&str, &'static str, u64) + Send + Sync;
type OldestReadyAgeCallback = dyn Fn(&str, StdDuration) + Send + Sync;
type JobDurationCallback = dyn Fn(&str, &'static str, StdDuration) + Send + Sync;
type RedisWakeupFailureCallback = dyn Fn() + Send + Sync;

/// 使用回调把任务监控事件适配到应用层指标实现。
#[derive(Clone)]
pub struct CallbackJobMetricsObserver {
    on_queue_depth: Arc<QueueDepthCallback>,
    on_oldest_ready_age: Arc<OldestReadyAgeCallback>,
    on_duration: Arc<JobDurationCallback>,
}

impl CallbackJobMetricsObserver {
    /// 创建由应用层回调驱动的任务监控观察者。
    pub fn new(
        on_queue_depth: Arc<QueueDepthCallback>,
        on_oldest_ready_age: Arc<OldestReadyAgeCallback>,
        on_duration: Arc<JobDurationCallback>,
    ) -> Self {
        Self {
            on_queue_depth,
            on_oldest_ready_age,
            on_duration,
        }
    }
}

impl JobMetricsObserver for CallbackJobMetricsObserver {
    fn set_queue_depth(&self, job_type: &str, status: &'static str, depth: u64) {
        (self.on_queue_depth)(job_type, status, depth);
    }

    fn set_oldest_ready_age(&self, job_type: &str, age: StdDuration) {
        (self.on_oldest_ready_age)(job_type, age);
    }

    fn observe_duration(&self, job_type: &str, result: &'static str, duration: StdDuration) {
        (self.on_duration)(job_type, result, duration);
    }
}

/// 后台任务分页列表的业务查询参数。
#[derive(Clone, Debug)]
pub struct BackgroundJobListParams {
    pub page: PageQuery,
    pub job_type: Option<String>,
    pub status: Option<String>,
}

/// 面向管理端的后台任务安全视图。
///
/// 任务载荷可能包含业务敏感字段，因此监控列表不会返回 `payload`。
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BackgroundJobVo {
    pub id: String,
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
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
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
}

impl JobQueue {
    /// 使用主库构造任务队列。所有领取、状态迁移和入队都必须走主库。
    pub fn new(database: DatabaseCluster) -> Self {
        Self {
            database,
            repository: Arc::new(BackgroundJobRepository),
            metrics_observer: Arc::new(RwLock::new(None)),
        }
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
        let now = self.database_now().await?;
        for job_type in job_types {
            let filter = BackgroundJobFilter {
                job_type: Some(job_type),
                ..Default::default()
            };
            let stats = self
                .repository
                .stats_filtered(self.primary(), filter.clone(), now)
                .await?;
            observer.set_queue_depth(job_type, "pending", stats.pending);
            observer.set_queue_depth(job_type, "running", stats.running);
            observer.set_queue_depth(job_type, "dead", stats.dead);
            observer.set_queue_depth(job_type, "ready", stats.ready);
            let oldest_ready_age = self
                .repository
                .oldest_ready_age(self.primary(), filter, now)
                .await?
                .unwrap_or_default();
            observer.set_oldest_ready_age(job_type, oldest_ready_age);
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
        self.repository
            .enqueue(self.database.write(), command, now)
            .await
    }

    /// 在既有业务事务中写入任务，保证业务数据和任务记录一起提交或回滚。
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

    /// 将操作日志写入持久化任务队列，避免响应返回后因进程退出而丢失日志。
    pub async fn enqueue_oper_log(
        &self,
        tenant_id: String,
        command: RecordOperLogCommand,
    ) -> AppResult<EnqueueBackgroundJobResult> {
        ryframe_core::validate_explicit_tenant(&tenant_id)?;
        let now = self.database_now().await?;
        let payload = serde_json::to_value(OperLogJobPayload {
            tenant_id: tenant_id.clone(),
            command,
        })
        .map_err(|error| AppError::Internal(format!("操作日志任务序列化失败: {error}")))?;
        self.repository
            .enqueue(
                self.database.write(),
                EnqueueBackgroundJob {
                    tenant_id: Some(tenant_id.clone()),
                    job_type: OPER_LOG_JOB_TYPE.to_owned(),
                    payload,
                    priority: 0,
                    available_at: now,
                    max_attempts: 5,
                    dedupe_key: None,
                    traceparent: crate::trace_context::current_traceparent(),
                },
                now,
            )
            .await
    }

    /// 按 UTC 自然日幂等写入一次消息过期清理任务。
    pub async fn enqueue_message_retention(&self) -> AppResult<EnqueueBackgroundJobResult> {
        let now = self.database_now().await?;
        let day = now.format("%F").to_string();
        self.repository
            .enqueue(
                self.database.write(),
                EnqueueBackgroundJob {
                    tenant_id: None,
                    job_type: MESSAGE_RETENTION_JOB_TYPE.to_owned(),
                    payload: serde_json::json!({ "run_date": day }),
                    priority: -10,
                    available_at: now,
                    max_attempts: 20,
                    dedupe_key: Some(format!("message:retention:{day}")),
                    traceparent: crate::trace_context::current_traceparent(),
                },
                now,
            )
            .await
    }

    /// 按 UTC 自然日幂等写入一次导出结果清理任务。
    pub async fn enqueue_export_cleanup(&self) -> AppResult<EnqueueBackgroundJobResult> {
        let now = self.database_now().await?;
        let day = now.format("%F").to_string();
        self.repository
            .enqueue(
                self.database.write(),
                EnqueueBackgroundJob {
                    tenant_id: None,
                    job_type: EXPORT_CLEANUP_JOB_TYPE.to_owned(),
                    payload: serde_json::json!({ "run_date": day }),
                    priority: -10,
                    available_at: now,
                    max_attempts: 20,
                    dedupe_key: Some(format!("export:cleanup:{day}")),
                    traceparent: crate::trace_context::current_traceparent(),
                },
                now,
            )
            .await
    }

    /// 查询当前租户的后台任务；任务类型和状态均为精确匹配。
    pub async fn list_for_tenant(
        &self,
        actor: &ActorContext,
        params: BackgroundJobListParams,
    ) -> AppResult<PageResult<BackgroundJobVo>> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let job_type = normalize_job_type_filter(params.job_type)?;
        let status = normalize_job_status_filter(params.status)?;
        let page = self
            .repository
            .list(
                self.primary(),
                BackgroundJobFilter {
                    tenant_id: Some(tenant_id),
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
        let now = self.database_now().await?;
        self.repository
            .stats_filtered(
                self.primary(),
                BackgroundJobFilter {
                    tenant_id: Some(tenant_id),
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
        let existing = self
            .repository
            .find_by_id_for_tenant(self.primary(), tenant_id, job_id)
            .await?
            .ok_or_else(|| AppError::NotFound("后台任务不存在或不属于当前租户".into()))?;
        if existing.status != background_job::Model::STATUS_DEAD {
            return Err(AppError::Conflict("仅允许重新投递死信任务".into()));
        }

        let now = self.database_now().await?;
        let retried = self
            .repository
            .retry_dead(self.primary(), tenant_id, job_id, now)
            .await?;
        if !retried {
            return Err(AppError::Conflict(
                "后台任务状态已变化，请刷新后重试".into(),
            ));
        }

        self.repository
            .find_by_id_for_tenant(self.primary(), tenant_id, job_id)
            .await?
            .map(BackgroundJobVo::from)
            .ok_or_else(|| AppError::Internal("后台任务重试后无法读取".into()))
    }

    fn repository(&self) -> &BackgroundJobRepository {
        &self.repository
    }

    fn has_metrics_observer(&self) -> bool {
        self.metrics_observer().is_some()
    }

    fn observe_job_duration(&self, job_type: &str, result: &'static str, duration: StdDuration) {
        if let Some(observer) = self.metrics_observer() {
            observer.observe_duration(job_type, result, duration);
        }
    }

    fn metrics_observer(&self) -> Option<Arc<dyn JobMetricsObserver>> {
        self.metrics_observer
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn primary(&self) -> &sea_orm::DatabaseConnection {
        self.database.write()
    }
}

/// 在启动时及每个 UTC 自然日创建消息保留与导出结果清理任务。
///
/// 多个 API 或 Worker 实例可同时运行该调度器；数据库中的幂等键会确保每天只保留一条任务。
pub fn spawn_message_retention_scheduler(
    queue: Arc<JobQueue>,
    mut shutdown: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if let Err(error) = queue.enqueue_message_retention().await {
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

/// 任务处理器。实现必须具备幂等性，因为 Worker 提供至少一次投递语义。
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// 返回唯一的任务类型标识。
    fn job_type(&self) -> &'static str;

    /// 执行已领取任务；返回错误将触发退避重试或死信。
    async fn handle(&self, job: &background_job::Model) -> AppResult<()>;
}

/// 单次 Worker 循环的结果。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum JobRunResult {
    /// 当前没有可执行任务。
    Idle,
    /// 任务成功完成。
    Succeeded,
    /// 任务被重新安排执行。
    Retried,
    /// 任务进入死信状态。
    Dead,
    /// 处理期间租约已失效，结果不再具有最终性。
    LeaseLost,
}

/// 负责领取、执行和确认任务的 Worker。
#[derive(Clone)]
pub struct JobWorker {
    queue: Arc<JobQueue>,
    handlers: Arc<BTreeMap<String, Arc<dyn JobHandler>>>,
    worker_prefix: String,
    lease_duration: Duration,
    poll_interval: StdDuration,
    concurrency: usize,
}

impl JobWorker {
    /// 根据运行配置创建 Worker。处理器需要通过 `with_handler` 显式注册。
    pub fn new(queue: Arc<JobQueue>, config: &JobConfig) -> AppResult<Self> {
        let lease_seconds = i64::try_from(config.lease_seconds)
            .map_err(|_| AppError::Config("jobs.lease_seconds 超出支持范围".into()))?;
        let worker_prefix = config
            .worker_id
            .clone()
            .unwrap_or_else(|| "ryframe-worker".into());
        Ok(Self {
            queue,
            handlers: Arc::new(BTreeMap::new()),
            worker_prefix,
            lease_duration: Duration::seconds(lease_seconds),
            poll_interval: StdDuration::from_millis(config.poll_interval_ms),
            concurrency: config.concurrency,
        })
    }

    /// 注册处理器；重复类型属于启动配置错误。
    pub fn with_handler(mut self, handler: Arc<dyn JobHandler>) -> AppResult<Self> {
        let handlers = Arc::make_mut(&mut self.handlers);
        let job_type = handler.job_type().to_owned();
        if handlers.insert(job_type.clone(), handler).is_some() {
            return Err(AppError::Config(format!(
                "后台任务处理器重复注册: {job_type}"
            )));
        }
        Ok(self)
    }

    /// 启动配置数量的并行消费循环，并在收到关闭信号后有序退出。
    pub fn spawn(self, shutdown: watch::Receiver<bool>) -> Vec<JoinHandle<()>> {
        let instance = Uuid::new_v4().simple().to_string();
        let mut tasks = (0..self.concurrency)
            .map(|slot| {
                let worker = self.clone();
                let worker_id = format!("{}-{slot}-{}", worker.worker_prefix, &instance[..12]);
                let receiver = shutdown.clone();
                tokio::spawn(async move {
                    worker.run_until_shutdown(worker_id, receiver).await;
                })
            })
            .collect::<Vec<_>>();

        if self.queue.has_metrics_observer() && !self.handlers.is_empty() {
            let queue = self.queue.clone();
            let job_types = self.handlers.keys().cloned().collect::<Vec<_>>();
            let mut receiver = shutdown.clone();
            tasks.push(tokio::spawn(async move {
                loop {
                    if let Err(error) = queue.report_metrics_for_types(&job_types).await {
                        tracing::warn!(%error, "后台任务队列指标采集失败");
                    }
                    tokio::select! {
                        _ = time::sleep(StdDuration::from_secs(30)) => {}
                        changed = receiver.changed() => {
                            if changed.is_err() || *receiver.borrow() {
                                break;
                            }
                        }
                    }
                }
            }));
        }

        let worker = self.clone();
        tasks.push(tokio::spawn(async move {
            worker.recover_expired_leases_until_shutdown(shutdown).await;
        }));

        tasks
    }

    /// 执行一次领取和处理，用于测试及自定义运行器。
    pub async fn run_once(&self, worker_id: &str) -> AppResult<JobRunResult> {
        let now = self.queue.database_now().await?;
        let Some(job) = self
            .queue
            .repository()
            .claim_next(self.queue.primary(), worker_id, self.lease_duration, now)
            .await?
        else {
            return Ok(JobRunResult::Idle);
        };

        let job_type = job.job_type.clone();
        let metric_job_type =
            bounded_job_type_label(self.handlers.contains_key(&job_type), &job_type);
        let started = std::time::Instant::now();
        let result = self.run_claimed_job(job, worker_id).await;
        self.queue.observe_job_duration(
            metric_job_type,
            job_run_result_label(&result),
            started.elapsed(),
        );
        result
    }

    /// 处理已完成租约领取的任务，并保留原有状态转换语义。
    async fn run_claimed_job(
        &self,
        job: background_job::Model,
        worker_id: &str,
    ) -> AppResult<JobRunResult> {
        let Some(handler) = self.handlers.get(&job.job_type).cloned() else {
            let now = self.queue.database_now().await?;
            let completed = self
                .queue
                .repository()
                .dead_letter(
                    self.queue.primary(),
                    job.id,
                    worker_id,
                    &format!("未注册任务处理器: {}", job.job_type),
                    now,
                )
                .await?;
            return Ok(if completed {
                JobRunResult::Dead
            } else {
                JobRunResult::LeaseLost
            });
        };

        let span = tracing::info_span!("background_job", job_type = %job.job_type);
        let _ = span.set_parent(crate::trace_context::extract_parent_context(
            job.traceparent.as_deref(),
        ));
        async {
            match handler.handle(&job).await {
                Ok(()) => {
                    let now = self.queue.database_now().await?;
                    let completed = self
                        .queue
                        .repository()
                        .complete(self.queue.primary(), job.id, worker_id, now)
                        .await?;
                    Ok(if completed {
                        JobRunResult::Succeeded
                    } else {
                        JobRunResult::LeaseLost
                    })
                }
                Err(error) => {
                    let now = self.queue.database_now().await?;
                    let retry_at = now + retry_delay(job.attempts);
                    match self
                        .queue
                        .repository()
                        .fail(
                            self.queue.primary(),
                            job.id,
                            worker_id,
                            retry_at,
                            &error.to_string(),
                            now,
                        )
                        .await?
                    {
                        JobFailureDisposition::Retried { .. } => Ok(JobRunResult::Retried),
                        JobFailureDisposition::Dead => Ok(JobRunResult::Dead),
                        JobFailureDisposition::LeaseLost => Ok(JobRunResult::LeaseLost),
                    }
                }
            }
        }
        .instrument(span)
        .await
    }

    async fn run_until_shutdown(&self, worker_id: String, mut shutdown: watch::Receiver<bool>) {
        tracing::info!(worker_id = %worker_id, "后台任务 Worker 已启动");
        let mut consecutive_infrastructure_failures = 0_u32;
        loop {
            if *shutdown.borrow() {
                break;
            }

            match self.run_once(&worker_id).await {
                Ok(JobRunResult::Idle) => {
                    consecutive_infrastructure_failures = 0;
                    tokio::select! {
                        _ = time::sleep(self.poll_interval) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                    }
                }
                Ok(JobRunResult::LeaseLost) => {
                    consecutive_infrastructure_failures = 0;
                    tracing::warn!(worker_id = %worker_id, "后台任务租约已失效，忽略本次处理结果");
                }
                Ok(_) => {
                    consecutive_infrastructure_failures = 0;
                }
                Err(error) => {
                    consecutive_infrastructure_failures =
                        consecutive_infrastructure_failures.saturating_add(1);
                    let delay = infrastructure_retry_delay(
                        self.poll_interval,
                        consecutive_infrastructure_failures,
                    );
                    tracing::warn!(
                        worker_id = %worker_id,
                        error = %error,
                        consecutive_infrastructure_failures,
                        delay_ms = delay.as_millis(),
                        "后台任务 Worker 基础设施调用失败，将退避后重试"
                    );
                    tokio::select! {
                        _ = time::sleep(delay) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        tracing::info!(worker_id = %worker_id, "后台任务 Worker 已停止");
    }

    /// 单独回收过期租约，避免与并发领取任务共享同一事务。
    async fn recover_expired_leases_until_shutdown(&self, mut shutdown: watch::Receiver<bool>) {
        loop {
            if *shutdown.borrow() {
                break;
            }
            if let Err(error) = self.queue.recover_expired_leases().await {
                tracing::warn!(%error, "后台任务过期租约回收失败，将在下次轮询重试");
            }
            tokio::select! {
                _ = time::sleep(self.poll_interval) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

/// 将任务执行结果映射为固定的低基数指标标签。
/// 单次 Outbox 投递循环的结果。
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum OutboxRunResult {
    Idle,
    Published,
    Retried,
    Dead,
    LeaseLost,
}

/// 将事务 Outbox 事件可靠转换为持久化后台任务的 Worker。
///
/// 当前仅投递消息发布事件；下游后台任务依旧通过自身的幂等键防止重复创建。
#[derive(Clone)]
pub struct OutboxWorker {
    queue: Arc<JobQueue>,
    repository: Arc<OutboxEventRepository>,
    worker_prefix: String,
    lease_duration: Duration,
    poll_interval: StdDuration,
    concurrency: usize,
}

impl OutboxWorker {
    /// 根据后台任务配置构建 Outbox Worker，复用相同的租约、轮询与并发策略。
    pub fn new(queue: Arc<JobQueue>, config: &JobConfig) -> AppResult<Self> {
        let lease_seconds = i64::try_from(config.lease_seconds)
            .map_err(|_| AppError::Config("jobs.lease_seconds 超出支持范围".into()))?;
        Ok(Self {
            queue,
            repository: Arc::new(OutboxEventRepository),
            worker_prefix: config
                .worker_id
                .clone()
                .unwrap_or_else(|| "ryframe-outbox".into()),
            lease_duration: Duration::seconds(lease_seconds),
            poll_interval: StdDuration::from_millis(config.poll_interval_ms),
            concurrency: config.concurrency,
        })
    }

    /// 启动多个 Outbox 消费循环，并在收到关闭信号后有序退出。
    pub fn spawn(self, shutdown: watch::Receiver<bool>) -> Vec<JoinHandle<()>> {
        let instance = Uuid::new_v4().simple().to_string();
        let mut tasks = (0..self.concurrency)
            .map(|slot| {
                let worker = self.clone();
                let worker_id = format!("{}-{slot}-{}", worker.worker_prefix, &instance[..12]);
                let receiver = shutdown.clone();
                tokio::spawn(async move {
                    worker.run_until_shutdown(worker_id, receiver).await;
                })
            })
            .collect::<Vec<_>>();
        let worker = self.clone();
        tasks.push(tokio::spawn(async move {
            worker.recover_expired_leases_until_shutdown(shutdown).await;
        }));
        tasks
    }

    /// 执行一次领取和投递，供测试及自定义运行器使用。
    pub async fn run_once(&self, worker_id: &str) -> AppResult<OutboxRunResult> {
        let now = self
            .repository
            .database_utc_now(self.queue.primary())
            .await?;
        let Some(event) = self
            .repository
            .claim_next(self.queue.primary(), worker_id, self.lease_duration, now)
            .await?
        else {
            return Ok(OutboxRunResult::Idle);
        };
        let span = tracing::info_span!("outbox_event", event_type = %event.event_type);
        let _ = span.set_parent(crate::trace_context::extract_parent_context(
            event.traceparent.as_deref(),
        ));
        self.run_claimed_event(event, worker_id)
            .instrument(span)
            .await
    }

    async fn run_claimed_event(
        &self,
        event: outbox_event::Model,
        worker_id: &str,
    ) -> AppResult<OutboxRunResult> {
        let now = self
            .repository
            .database_utc_now(self.queue.primary())
            .await?;
        let delivery = self.publish_event_as_job(&event, worker_id, now).await;
        match delivery {
            Ok(true) => Ok(OutboxRunResult::Published),
            Ok(false) => Ok(OutboxRunResult::LeaseLost),
            Err(error) => {
                let now = self
                    .repository
                    .database_utc_now(self.queue.primary())
                    .await?;
                let retry_at = now + retry_delay(event.attempts);
                match self
                    .repository
                    .fail(
                        self.queue.primary(),
                        event.id,
                        worker_id,
                        retry_at,
                        &error.to_string(),
                        now,
                    )
                    .await?
                {
                    OutboxFailureDisposition::Retried { .. } => Ok(OutboxRunResult::Retried),
                    OutboxFailureDisposition::Dead => Ok(OutboxRunResult::Dead),
                    OutboxFailureDisposition::LeaseLost => Ok(OutboxRunResult::LeaseLost),
                }
            }
        }
    }

    async fn publish_event_as_job(
        &self,
        event: &outbox_event::Model,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let job_type = match event.event_type.as_str() {
            MESSAGE_PUBLISHED_OUTBOX_EVENT_TYPE => MESSAGE_DISPATCH_JOB_TYPE,
            _ => {
                return Err(AppError::Validation(format!(
                    "未注册 Outbox 事件处理器: {}",
                    event.event_type
                )));
            }
        };
        let transaction = self
            .queue
            .primary()
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let result = async {
            self.queue
                .repository()
                .enqueue_in_transaction(
                    &transaction,
                    EnqueueBackgroundJob {
                        tenant_id: event.tenant_id.clone(),
                        job_type: job_type.to_owned(),
                        payload: event.payload.clone(),
                        priority: 10,
                        available_at: now,
                        max_attempts: event.max_attempts,
                        dedupe_key: event.dedupe_key.clone(),
                        traceparent: event.traceparent.clone(),
                    },
                    now,
                )
                .await?;
            self.repository
                .mark_published_in_transaction(&transaction, event.id, worker_id, now)
                .await
        }
        .await;
        match result {
            Ok(true) => {
                transaction
                    .commit()
                    .await
                    .map_err(|error| AppError::Database(error.to_string()))?;
                Ok(true)
            }
            Ok(false) => {
                let _ = transaction.rollback().await;
                Ok(false)
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }

    async fn run_until_shutdown(&self, worker_id: String, mut shutdown: watch::Receiver<bool>) {
        tracing::info!(worker_id = %worker_id, "Outbox Worker 已启动");
        let mut consecutive_infrastructure_failures = 0_u32;
        loop {
            if *shutdown.borrow() {
                break;
            }
            match self.run_once(&worker_id).await {
                Ok(OutboxRunResult::Idle) => {
                    consecutive_infrastructure_failures = 0;
                    tokio::select! {
                        _ = time::sleep(self.poll_interval) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                    }
                }
                Ok(OutboxRunResult::LeaseLost) => {
                    consecutive_infrastructure_failures = 0;
                    tracing::warn!(worker_id = %worker_id, "Outbox 事件租约已失效，忽略本次投递结果");
                }
                Ok(_) => consecutive_infrastructure_failures = 0,
                Err(error) => {
                    consecutive_infrastructure_failures =
                        consecutive_infrastructure_failures.saturating_add(1);
                    let delay = infrastructure_retry_delay(
                        self.poll_interval,
                        consecutive_infrastructure_failures,
                    );
                    tracing::warn!(
                        worker_id = %worker_id,
                        error = %error,
                        consecutive_infrastructure_failures,
                        delay_ms = delay.as_millis(),
                        "Outbox Worker 基础设施调用失败，将退避后重试"
                    );
                    tokio::select! {
                        _ = time::sleep(delay) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        tracing::info!(worker_id = %worker_id, "Outbox Worker 已停止");
    }

    /// 单独回收过期租约，避免与并发领取事件共享同一事务。
    async fn recover_expired_leases_until_shutdown(&self, mut shutdown: watch::Receiver<bool>) {
        loop {
            if *shutdown.borrow() {
                break;
            }
            let recovery = async {
                let now = self
                    .repository
                    .database_utc_now(self.queue.primary())
                    .await?;
                self.repository
                    .recover_expired_leases(self.queue.primary(), now)
                    .await
            }
            .await;
            if let Err(error) = recovery {
                tracing::warn!(%error, "Outbox 过期租约回收失败，将在下次轮询重试");
            }
            tokio::select! {
                _ = time::sleep(self.poll_interval) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }
}

fn job_run_result_label(result: &AppResult<JobRunResult>) -> &'static str {
    match result {
        Ok(JobRunResult::Succeeded) => "succeeded",
        Ok(JobRunResult::Retried) => "retried",
        Ok(JobRunResult::Dead) => "dead",
        Ok(JobRunResult::LeaseLost) => "lease_lost",
        Ok(JobRunResult::Idle) => "idle",
        Err(_) => "error",
    }
}

/// 将未注册任务归并到固定标签，避免异常数据扩大 Prometheus 标签基数。
fn bounded_job_type_label(registered: bool, job_type: &str) -> &str {
    if registered { job_type } else { "unregistered" }
}

/// 操作日志任务的序列化载荷。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperLogJobPayload {
    tenant_id: String,
    command: RecordOperLogCommand,
}

/// 将持久化任务还原为操作日志记录。
pub struct OperLogJobHandler {
    service: Arc<OperLogService>,
}

impl OperLogJobHandler {
    /// 使用操作日志服务创建处理器。
    pub fn new(service: Arc<OperLogService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for OperLogJobHandler {
    fn job_type(&self) -> &'static str {
        OPER_LOG_JOB_TYPE
    }

    async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
        let payload: OperLogJobPayload = serde_json::from_value(job.payload.clone())
            .map_err(|error| AppError::Validation(format!("操作日志任务载荷无效: {error}")))?;
        self.service
            .record_for_tenant(&payload.tenant_id, payload.command)
            .await
    }
}

/// 执行对象存储导出并更新公开导出任务状态的处理器。
pub struct ExportJobHandler {
    service: Arc<ExportService>,
}

impl ExportJobHandler {
    pub fn new(service: Arc<ExportService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for ExportJobHandler {
    fn job_type(&self) -> &'static str {
        EXPORT_JOB_TYPE
    }

    async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
        match self.service.execute_background_job(job.id).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let terminal = matches!(
                    error,
                    AppError::Validation(_)
                        | AppError::Authentication(_)
                        | AppError::Authorization(_)
                        | AppError::NotFound(_)
                        | AppError::Conflict(_)
                        | AppError::PayloadTooLarge(_)
                );
                if let Err(record_error) = self
                    .service
                    .record_execution_failure(
                        job.id,
                        terminal || job.attempts >= job.max_attempts,
                        &error.to_string(),
                    )
                    .await
                {
                    tracing::error!(%record_error, job_id = job.id, "记录导出任务失败状态时发生错误");
                }
                if terminal {
                    tracing::warn!(%error, job_id = job.id, "导出任务因不可重试错误终止");
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
}

/// 清理过期导出文件的处理器。
pub struct ExportCleanupJobHandler {
    service: Arc<ExportService>,
}

impl ExportCleanupJobHandler {
    pub fn new(service: Arc<ExportService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl JobHandler for ExportCleanupJobHandler {
    fn job_type(&self) -> &'static str {
        EXPORT_CLEANUP_JOB_TYPE
    }

    async fn handle(&self, _job: &background_job::Model) -> AppResult<()> {
        let cleaned = self.service.cleanup_expired().await?;
        tracing::info!(cleaned, "已清理过期导出结果");
        Ok(())
    }
}

/// 消息投递任务的序列化载荷。
#[derive(Debug, Deserialize)]
struct MessageDispatchJobPayload {
    message_id: String,
}

/// 将消息投递任务交给消息中心服务处理。
pub struct MessageDispatchJobHandler {
    service: Arc<MessageService>,
    redis: Option<RedisClient>,
    on_redis_wakeup_failure: Arc<RedisWakeupFailureCallback>,
}

/// 清理到期消息及其级联收件箱记录的任务处理器。
pub struct MessageRetentionJobHandler {
    service: Arc<MessageService>,
    on_deleted: Arc<dyn Fn(u64) + Send + Sync>,
}

impl MessageRetentionJobHandler {
    /// 使用消息中心服务创建过期清理处理器。
    pub fn new(service: Arc<MessageService>) -> Self {
        Self {
            service,
            on_deleted: Arc::new(|_| {}),
        }
    }

    /// 注入删除计数观察器，使传输层可记录指标而不反向依赖中间件 crate。
    pub fn with_deleted_observer(mut self, observer: Arc<dyn Fn(u64) + Send + Sync>) -> Self {
        self.on_deleted = observer;
        self
    }
}

#[async_trait]
impl JobHandler for MessageRetentionJobHandler {
    fn job_type(&self) -> &'static str {
        MESSAGE_RETENTION_JOB_TYPE
    }

    async fn handle(&self, _job: &background_job::Model) -> AppResult<()> {
        let deleted = self.service.delete_expired().await?;
        (self.on_deleted)(deleted);
        tracing::info!(deleted, "已完成过期消息清理");
        Ok(())
    }
}

impl MessageDispatchJobHandler {
    /// 使用消息中心服务和可选 Redis 唤醒通道创建处理器。
    ///
    /// Redis 只用于降低在线投递延迟；未配置 Redis 时，客户端仍会通过收件箱补拉消息。
    pub fn new(service: Arc<MessageService>, redis: Option<RedisClient>) -> Self {
        Self {
            service,
            redis,
            on_redis_wakeup_failure: Arc::new(|| {}),
        }
    }

    /// 注入 Redis 唤醒失败观察器，使组合根能够记录运行时降级指标。
    pub fn with_redis_wakeup_failure_observer(
        mut self,
        observer: Arc<RedisWakeupFailureCallback>,
    ) -> Self {
        self.on_redis_wakeup_failure = observer;
        self
    }
}

#[async_trait]
impl JobHandler for MessageDispatchJobHandler {
    fn job_type(&self) -> &'static str {
        MESSAGE_DISPATCH_JOB_TYPE
    }

    async fn handle(&self, job: &background_job::Model) -> AppResult<()> {
        let payload: MessageDispatchJobPayload = serde_json::from_value(job.payload.clone())
            .map_err(|error| AppError::Validation(format!("消息投递任务载荷无效: {error}")))?;
        let message_id = payload
            .message_id
            .parse::<i64>()
            .map_err(|_| AppError::Validation("消息投递任务的 message_id 无效".into()))?;
        self.service.dispatch(message_id).await?;
        if let Some(redis) = &self.redis
            && let Err(error) = redis
                .publish(MESSAGE_DISPATCH_REDIS_CHANNEL, message_id.to_string())
                .await
        {
            report_redis_wakeup_failure(error, self.on_redis_wakeup_failure.as_ref());
        }
        Ok(())
    }
}

/// Redis 唤醒只影响实时投递延迟；持久化收件箱已经完成，失败后由客户端补拉。
fn report_redis_wakeup_failure(
    error: impl std::fmt::Display,
    observer: &RedisWakeupFailureCallback,
) {
    observer();
    tracing::warn!(%error, "消息 Redis 唤醒失败，客户端将通过收件箱补拉");
}

/// 生成指数退避等待时间，最高五分钟。
fn retry_delay(attempts: i32) -> Duration {
    let exponent = attempts.saturating_sub(1).clamp(0, 6) as u32;
    let seconds = 5_i64.saturating_mul(1_i64 << exponent).min(300);
    Duration::seconds(seconds)
}

/// 计算基础设施调用连续失败后的本地退避时间，避免数据库故障时形成高频请求和日志。
fn infrastructure_retry_delay(
    poll_interval: StdDuration,
    consecutive_failures: u32,
) -> StdDuration {
    const MAX_DELAY: StdDuration = StdDuration::from_secs(30);
    let exponent = consecutive_failures.saturating_sub(1).min(30);
    let multiplier = 1_u32 << exponent;
    poll_interval.saturating_mul(multiplier).min(MAX_DELAY)
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

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use ryframe_kernel::AppError;

    use super::{
        JobRunResult, bounded_job_type_label, infrastructure_retry_delay, job_run_result_label,
        normalize_job_status_filter, normalize_job_type_filter, report_redis_wakeup_failure,
        retry_delay,
    };

    #[test]
    fn retry_delay_is_bounded_exponential_backoff() {
        assert_eq!(retry_delay(1).num_seconds(), 5);
        assert_eq!(retry_delay(2).num_seconds(), 10);
        assert_eq!(retry_delay(7).num_seconds(), 300);
        assert_eq!(retry_delay(99).num_seconds(), 300);
    }

    #[test]
    fn infrastructure_retry_delay_is_bounded_exponential_backoff() {
        let poll_interval = std::time::Duration::from_millis(50);
        assert_eq!(infrastructure_retry_delay(poll_interval, 1), poll_interval);
        assert_eq!(
            infrastructure_retry_delay(poll_interval, 2),
            std::time::Duration::from_millis(100)
        );
        assert_eq!(
            infrastructure_retry_delay(poll_interval, 11),
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            infrastructure_retry_delay(std::time::Duration::from_secs(60), 1),
            std::time::Duration::from_secs(30)
        );
    }

    #[test]
    fn redis_wakeup_failure_is_observed_without_propagating_an_error() {
        let failures = Arc::new(AtomicUsize::new(0));
        let observer_failures = failures.clone();
        report_redis_wakeup_failure("redis unavailable", &move || {
            observer_failures.fetch_add(1, Ordering::Relaxed);
        });

        assert_eq!(failures.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn monitoring_filters_accept_only_known_values() {
        assert_eq!(
            normalize_job_type_filter(Some(" system.audit ".into())).unwrap(),
            Some("system.audit".into())
        );
        assert!(normalize_job_type_filter(Some(String::new())).is_err());
        assert_eq!(
            normalize_job_status_filter(Some("dead".into())).unwrap(),
            Some("dead".into())
        );
        assert!(normalize_job_status_filter(Some("cancelled".into())).is_err());
    }

    #[test]
    fn execution_results_use_bounded_metric_labels() {
        assert_eq!(
            job_run_result_label(&Ok(JobRunResult::Succeeded)),
            "succeeded"
        );
        assert_eq!(job_run_result_label(&Ok(JobRunResult::Retried)), "retried");
        assert_eq!(job_run_result_label(&Ok(JobRunResult::Dead)), "dead");
        assert_eq!(
            job_run_result_label(&Ok(JobRunResult::LeaseLost)),
            "lease_lost"
        );
        assert_eq!(
            job_run_result_label(&Err(AppError::Internal("failure".into()))),
            "error"
        );
        assert_eq!(
            bounded_job_type_label(true, "system.message.dispatch"),
            "system.message.dispatch"
        );
        assert_eq!(
            bounded_job_type_label(false, "unexpected.user_supplied_type"),
            "unregistered"
        );
    }
}
