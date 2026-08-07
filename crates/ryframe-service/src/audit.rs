use std::{
    future::Future,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use ryframe_db::{DatabaseCluster, OutboxEventRepository, RecordOutboxEvent};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{DatabaseTransaction, TransactionTrait};
use serde::{Deserialize, Serialize};

use crate::jobs::JobQueue;
use crate::system::{OperLogStatus, RecordOperLogCommand};

/// 操作审计事件在事务 Outbox 中使用的稳定类型标识。
pub const AUDIT_OPERATION_OUTBOX_EVENT_TYPE: &str = "audit.operation";

const AUDIT_AGGREGATE_TYPE: &str = "operation_audit";
const OUTBOX_MAX_ATTEMPTS: i32 = 20;

static AUDIT_FAILURE_HOOK: OnceLock<fn(&'static str)> = OnceLock::new();

tokio::task_local! {
    static CURRENT_AUDIT_REQUEST: AuditRequestContext;
}

/// 安装进程级审计失败观测钩子；重复安装不会改变已经生效的钩子。
pub fn set_audit_failure_hook(hook: fn(&'static str)) {
    let _ = AUDIT_FAILURE_HOOK.set(hook);
}

/// 记录一个取值受控的审计失败阶段。
pub fn record_audit_failure(stage: &'static str) {
    if let Some(hook) = AUDIT_FAILURE_HOOK.get() {
        hook(stage);
    }
}

/// Outbox 中持久化的操作审计载荷。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AuditOperationEvent {
    pub event_id: String,
    pub request_id: String,
    pub tenant_id: String,
    pub command: RecordOperLogCommand,
}

#[derive(Debug)]
struct AuditRequestState {
    transaction_bound: AtomicBool,
    transaction_committed: AtomicBool,
}

/// 一个写请求在调用链中的审计上下文。
#[derive(Clone, Debug)]
pub struct AuditRequestContext {
    event_id: String,
    request_id: String,
    tenant_id: String,
    base_command: RecordOperLogCommand,
    started_at: Instant,
    state: Arc<AuditRequestState>,
}

impl AuditRequestContext {
    /// 创建由 HTTP 中间件向业务调用链传播的审计上下文。
    pub fn new(
        event_id: String,
        request_id: String,
        tenant_id: String,
        base_command: RecordOperLogCommand,
    ) -> AppResult<Self> {
        validate_identifier("event_id", &event_id)?;
        validate_identifier("request_id", &request_id)?;
        ryframe_core::validate_explicit_tenant(&tenant_id)?;
        Ok(Self {
            event_id,
            request_id,
            tenant_id,
            base_command,
            started_at: Instant::now(),
            state: Arc::new(AuditRequestState {
                transaction_bound: AtomicBool::new(false),
                transaction_committed: AtomicBool::new(false),
            }),
        })
    }

    /// 根据最终请求结果生成不可变审计事件。
    pub fn event(&self, status: OperLogStatus, error_msg: Option<String>) -> AuditOperationEvent {
        let mut command = self.base_command.clone();
        command.status = status;
        command.error_msg = error_msg;
        command.cost_time = elapsed_millis(self.started_at);
        AuditOperationEvent {
            event_id: self.event_id.clone(),
            request_id: self.request_id.clone(),
            tenant_id: self.tenant_id.clone(),
            command,
        }
    }

    /// 判断业务事务是否已经把审计事件与业务变更一起提交。
    pub fn transaction_committed(&self) -> bool {
        self.state.transaction_committed.load(Ordering::Acquire)
    }

    /// 判断业务代码是否尝试过事务绑定，可用于区分缺失接入与事务回滚。
    pub fn transaction_bound(&self) -> bool {
        self.state.transaction_bound.load(Ordering::Acquire)
    }
}

/// 在当前异步调用链中传播审计上下文。
pub async fn scope_audit_request<F>(context: AuditRequestContext, future: F) -> F::Output
where
    F: Future,
{
    CURRENT_AUDIT_REQUEST.scope(context, future).await
}

/// 业务事务审计绑定句柄。
///
/// 调用方必须先提交数据库事务，再调用 `mark_committed`。事务回滚或提交失败时不要标记，
/// 中间件会写入独立的失败审计事件。
#[must_use = "数据库事务成功提交后必须调用 mark_committed"]
pub struct AuditTransactionBinding {
    context: AuditRequestContext,
}

impl AuditTransactionBinding {
    /// 标记承载业务变更与 Outbox 事件的事务已经提交。
    pub fn mark_committed(self) {
        self.context
            .state
            .transaction_committed
            .store(true, Ordering::Release);
    }
}

/// 把当前请求的成功审计事件写入调用方业务事务。
///
/// 非 HTTP 调用链返回 `None`。HTTP 写请求返回绑定句柄，调用方需在事务提交成功后标记。
pub async fn record_current_audit_in_transaction(
    transaction: &DatabaseTransaction,
) -> AppResult<Option<AuditTransactionBinding>> {
    let Some(context) = CURRENT_AUDIT_REQUEST.try_with(Clone::clone).ok() else {
        return Ok(None);
    };
    context
        .state
        .transaction_bound
        .store(true, Ordering::Release);
    let event = context.event(OperLogStatus::Success, None);
    record_event_in_transaction(transaction, &event, OUTBOX_MAX_ATTEMPTS).await?;
    Ok(Some(AuditTransactionBinding { context }))
}

/// 在提交调用方事务前自动写入当前请求审计事件，并在提交成功后完成绑定标记。
///
/// 业务服务可以用该函数直接替换裸 `transaction.commit()`，避免遗漏 `mark_committed`。
pub async fn commit_current_audit(transaction: DatabaseTransaction) -> AppResult<()> {
    let binding = record_current_audit_in_transaction(&transaction).await?;
    transaction.commit().await.map_err(database_error)?;
    if let Some(binding) = binding {
        binding.mark_committed();
    }
    Ok(())
}

/// 由 HTTP 边界使用的独立短事务 Outbox 写入器。
#[derive(Clone)]
pub struct AuditOutbox {
    database: DatabaseCluster,
    max_attempts: i32,
    job_queue: Option<Arc<JobQueue>>,
}

impl AuditOutbox {
    pub fn new(database: DatabaseCluster, default_max_attempts: i32) -> Self {
        Self {
            database,
            max_attempts: default_max_attempts.clamp(1, OUTBOX_MAX_ATTEMPTS),
            job_queue: None,
        }
    }

    /// 连接共享队列，使独立 Outbox 事务提交后可以发送可选唤醒提示。
    pub fn with_job_queue(mut self, job_queue: Arc<JobQueue>) -> Self {
        self.job_queue = Some(job_queue);
        self
    }

    /// 使用独立短事务持久化审计事件，响应成功与否均不会依赖内存任务。
    pub async fn record(&self, event: &AuditOperationEvent) -> AppResult<()> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        let result = record_event_in_transaction(&transaction, event, self.max_attempts).await;
        match result {
            Ok(()) => {
                transaction.commit().await.map_err(database_error)?;
                if let Some(job_queue) = &self.job_queue {
                    job_queue.notify_outbox().await;
                }
                Ok(())
            }
            Err(error) => {
                let _ = transaction.rollback().await;
                Err(error)
            }
        }
    }
}

async fn record_event_in_transaction(
    transaction: &DatabaseTransaction,
    event: &AuditOperationEvent,
    max_attempts: i32,
) -> AppResult<()> {
    validate_identifier("event_id", &event.event_id)?;
    validate_identifier("request_id", &event.request_id)?;
    ryframe_core::validate_explicit_tenant(&event.tenant_id)?;
    let now = OutboxEventRepository.database_utc_now(transaction).await?;
    let payload = serde_json::to_value(event)
        .map_err(|error| AppError::Internal(format!("审计事件序列化失败: {error}")))?;
    let trace_context = crate::trace_context::current_trace_context();
    OutboxEventRepository
        .record_in_transaction(
            transaction,
            RecordOutboxEvent {
                tenant_id: Some(event.tenant_id.clone()),
                event_type: AUDIT_OPERATION_OUTBOX_EVENT_TYPE.to_owned(),
                aggregate_type: AUDIT_AGGREGATE_TYPE.to_owned(),
                aggregate_id: event.event_id.clone(),
                payload,
                available_at: now,
                max_attempts,
                dedupe_key: Some(event.event_id.clone()),
                traceparent: trace_context.traceparent,
                tracestate: trace_context.tracestate,
            },
            now,
        )
        .await?;
    Ok(())
}

fn validate_identifier(name: &str, value: &str) -> AppResult<()> {
    if value.is_empty() || value.len() > 36 {
        return Err(AppError::Validation(format!(
            "{name} 长度必须介于 1 和 36 之间"
        )));
    }
    Ok(())
}

fn elapsed_millis(started_at: Instant) -> i64 {
    i64::try_from(started_at.elapsed().as_millis()).unwrap_or(i64::MAX)
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
