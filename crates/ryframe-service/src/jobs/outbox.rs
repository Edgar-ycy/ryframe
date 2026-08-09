use std::{sync::Arc, time::Duration as StdDuration};

use chrono::{DateTime, Duration, Utc};
use ryframe_config::JobConfig;
use ryframe_db::{
    EnqueueBackgroundJob, OutboxEventRepository, OutboxFailureDisposition, outbox_event,
};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::TransactionTrait;
use tokio::{sync::watch, task::JoinHandle, time};
use tracing::Instrument;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

use super::backoff::{jittered_delay, next_idle_wait};
use super::worker::{infrastructure_retry_delay, retry_delay};
use super::{MESSAGE_PUBLISHED_OUTBOX_EVENT_TYPE, queue::JobQueue};
use crate::system::{MESSAGE_DISPATCH_JOB_TYPE, OperLogService};
use crate::{
    AUDIT_OPERATION_OUTBOX_EVENT_TYPE, AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE, AuditOperationEvent,
    AuthorizationCache, AuthorizationMirrorUpdate, record_audit_failure,
};

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
    max_idle_poll_interval: StdDuration,
    lease_recovery_interval: StdDuration,
    concurrency: usize,
    authorization_cache: AuthorizationCache,
    audit_service: Option<Arc<OperLogService>>,
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
            max_idle_poll_interval: StdDuration::from_millis(config.max_idle_poll_interval_ms),
            lease_recovery_interval: StdDuration::from_secs(config.lease_recovery_interval_seconds),
            concurrency: config.concurrency,
            authorization_cache: AuthorizationCache::disabled(),
            audit_service: None,
        })
    }

    /// 注入授权版本镜像修复器；生产 Worker 必须与 API 使用相同 Redis 配置。
    pub fn with_authorization_cache(mut self, authorization_cache: AuthorizationCache) -> Self {
        self.authorization_cache = authorization_cache;
        self
    }

    /// 注入操作审计落库服务。
    pub fn with_audit_service(mut self, audit_service: Arc<OperLogService>) -> Self {
        self.audit_service = Some(audit_service);
        self
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
        if let Some(listener) = self.queue.spawn_wakeup_listener(shutdown.clone()) {
            tasks.push(listener);
        }
        let worker = self.clone();
        tasks.push(tokio::spawn(async move {
            worker.recover_expired_leases_until_shutdown(shutdown).await;
        }));
        tasks
    }

    /// 执行一次领取和投递，供单次执行模式及自定义运行器使用。
    pub async fn run_once(&self, worker_id: &str) -> AppResult<OutboxRunResult> {
        let now = match self.repository.database_utc_now(self.queue.primary()).await {
            Ok(now) => now,
            Err(error) => {
                self.queue.record_claim_attempt("outbox", "error");
                return Err(error);
            }
        };
        let event = match self
            .repository
            .claim_next(self.queue.primary(), worker_id, self.lease_duration, now)
            .await
        {
            Ok(Some(event)) => {
                self.queue.record_claim_attempt("outbox", "claimed");
                event
            }
            Ok(None) => {
                self.queue.record_claim_attempt("outbox", "idle");
                return Ok(OutboxRunResult::Idle);
            }
            Err(error) => {
                self.queue.record_claim_attempt("outbox", "error");
                return Err(error);
            }
        };
        let span = tracing::info_span!("outbox_event", event_type = %event.event_type);
        let _ = span.set_parent(crate::trace_context::extract_parent_context(
            event.traceparent.as_deref(),
            event.tracestate.as_deref(),
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
                    OutboxFailureDisposition::Retried { available_at } => {
                        tracing::debug!(
                            outbox_event_id = event.id,
                            event_type = %event.event_type,
                            worker_id,
                            attempts = event.attempts,
                            max_attempts = event.max_attempts,
                            retry_at = %available_at,
                            error = %error,
                            "Outbox 事件投递失败，已安排重试"
                        );
                        Ok(OutboxRunResult::Retried)
                    }
                    OutboxFailureDisposition::Dead => {
                        tracing::error!(
                            outbox_event_id = event.id,
                            event_type = %event.event_type,
                            worker_id,
                            attempts = event.attempts,
                            max_attempts = event.max_attempts,
                            error = %error,
                            "Outbox 事件重试耗尽，已进入死信状态"
                        );
                        Ok(OutboxRunResult::Dead)
                    }
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
        if event.event_type == AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE {
            return self
                .repair_authorization_mirror(event, worker_id, now)
                .await;
        }
        if event.event_type == AUDIT_OPERATION_OUTBOX_EVENT_TYPE {
            return self.publish_audit_event(event, worker_id, now).await;
        }
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
                        schedule_id: None,
                        scheduled_for: Some(now),
                        max_runtime_seconds: None,
                        job_type: job_type.to_owned(),
                        payload: event.payload.clone(),
                        priority: 10,
                        available_at: now,
                        max_attempts: event.max_attempts,
                        dedupe_key: event.dedupe_key.clone(),
                        traceparent: event.traceparent.clone(),
                        tracestate: event.tracestate.clone(),
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
                self.queue.notify_background_jobs().await;
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

    async fn publish_audit_event(
        &self,
        event: &outbox_event::Model,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let service = self.audit_service.as_ref().ok_or_else(|| {
            record_audit_failure("handler_missing");
            AppError::Config("Outbox Worker 未配置操作审计服务".into())
        })?;
        let payload: AuditOperationEvent =
            serde_json::from_value(event.payload.clone()).map_err(|error| {
                record_audit_failure("payload_decode");
                AppError::Validation(format!("操作审计事件载荷无效: {error}"))
            })?;
        if event.tenant_id.as_deref() != Some(payload.tenant_id.as_str()) {
            record_audit_failure("tenant_mismatch");
            return Err(AppError::Authorization(
                "操作审计事件的租户与 Outbox 信封不一致".into(),
            ));
        }

        let transaction = self
            .queue
            .primary()
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let result = async {
            service
                .record_event_in_transaction(
                    &transaction,
                    &payload.event_id,
                    &payload.request_id,
                    &payload.tenant_id,
                    payload.command,
                )
                .await
                .inspect_err(|_| record_audit_failure("oper_log_write"))?;
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

    async fn repair_authorization_mirror(
        &self,
        event: &outbox_event::Model,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool> {
        let update: AuthorizationMirrorUpdate = serde_json::from_value(event.payload.clone())
            .map_err(|error| AppError::Validation(format!("授权镜像事件负载无效: {error}")))?;
        match update {
            AuthorizationMirrorUpdate::Tenant {
                tenant_id,
                authorization_epoch,
            } => {
                self.authorization_cache
                    .repair_tenant_epoch(&tenant_id, authorization_epoch)
                    .await?;
            }
            AuthorizationMirrorUpdate::User {
                tenant_id,
                user_id,
                authorization_version,
            } => {
                self.authorization_cache
                    .repair_user_version(&tenant_id, user_id, authorization_version)
                    .await?;
            }
            AuthorizationMirrorUpdate::TenantCacheNamespace {
                tenant_id,
                namespace,
                namespace_version,
            } => {
                self.authorization_cache
                    .repair_namespace_version(&tenant_id, &namespace, namespace_version)
                    .await?;
            }
        }

        // Redis 更新脚本是单调且幂等的；崩溃后重复执行不会覆盖更新版本。
        let transaction = self
            .queue
            .primary()
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let marked = self
            .repository
            .mark_published_in_transaction(&transaction, event.id, worker_id, now)
            .await?;
        if marked {
            transaction
                .commit()
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        } else {
            let _ = transaction.rollback().await;
        }
        Ok(marked)
    }

    async fn run_until_shutdown(&self, worker_id: String, mut shutdown: watch::Receiver<bool>) {
        tracing::info!(worker_id = %worker_id, "Outbox Worker 已启动");
        let mut consecutive_infrastructure_failures = 0_u32;
        let mut idle_wait = self.poll_interval;
        let mut wakeups = self.queue.subscribe_outbox_wakeups();
        loop {
            if *shutdown.borrow() {
                break;
            }
            match self.run_once(&worker_id).await {
                Ok(OutboxRunResult::Idle) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "Outbox Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                    idle_wait =
                        next_idle_wait(idle_wait, self.poll_interval, self.max_idle_poll_interval);
                    tokio::select! {
                        _ = time::sleep(jittered_delay(idle_wait)) => {}
                        changed = shutdown.changed() => {
                            if changed.is_err() || *shutdown.borrow() {
                                break;
                            }
                        }
                        changed = wakeups.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            idle_wait = self.poll_interval;
                        }
                    }
                }
                Ok(OutboxRunResult::LeaseLost) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "Outbox Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                    idle_wait = self.poll_interval;
                    tracing::warn!(worker_id = %worker_id, "Outbox 事件租约已失效，忽略本次投递结果");
                }
                Ok(_) => {
                    if consecutive_infrastructure_failures > 0 {
                        tracing::info!(worker_id = %worker_id, "Outbox Worker 基础设施调用已恢复");
                    }
                    consecutive_infrastructure_failures = 0;
                    idle_wait = self.poll_interval;
                }
                Err(error) => {
                    consecutive_infrastructure_failures =
                        consecutive_infrastructure_failures.saturating_add(1);
                    let delay = infrastructure_retry_delay(
                        self.poll_interval,
                        consecutive_infrastructure_failures,
                    );
                    if consecutive_infrastructure_failures == 1 {
                        tracing::warn!(
                            worker_id = %worker_id,
                            error = %error,
                            delay_ms = delay.as_millis(),
                            "Outbox Worker 基础设施调用失败，将退避后重试"
                        );
                    } else {
                        tracing::debug!(
                            worker_id = %worker_id,
                            error = %error,
                            consecutive_infrastructure_failures,
                            delay_ms = delay.as_millis(),
                            "Outbox Worker 基础设施调用仍不可用"
                        );
                    }
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
        let mut recovery_degraded = false;
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
            match recovery {
                Ok(_) if recovery_degraded => {
                    tracing::info!("Outbox 过期租约回收已恢复");
                    recovery_degraded = false;
                }
                Ok(_) => {}
                Err(error) if recovery_degraded => {
                    tracing::debug!(%error, "Outbox 过期租约回收仍不可用");
                }
                Err(error) => {
                    tracing::warn!(%error, "Outbox 过期租约回收失败，将在下次轮询重试");
                    recovery_degraded = true;
                }
            }
            tokio::select! {
                _ = time::sleep(self.lease_recovery_interval) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    }
}
