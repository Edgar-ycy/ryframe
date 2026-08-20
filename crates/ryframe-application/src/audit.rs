use std::{
    future::Future,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use ryframe_kernel::{AppError, AppResult};
use serde::{Deserialize, Serialize};

use crate::PersistenceFuture;
use crate::jobs::JobQueue;
use crate::system::{OperLogStatus, RecordOperLogCommand};

/// 操作审计事件在事务 Outbox 中使用的稳定类型标识。
pub const AUDIT_OPERATION_OUTBOX_EVENT_TYPE: &str = "audit.operation";

pub(crate) const AUDIT_AGGREGATE_TYPE: &str = "operation_audit";
pub(crate) const OUTBOX_MAX_ATTEMPTS: i32 = 20;

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
        crate::enforce_tenant_scope(&tenant_id)?;
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

pub(crate) fn bind_current_audit() -> Option<(AuditOperationEvent, AuditTransactionBinding)> {
    let context = CURRENT_AUDIT_REQUEST.try_with(Clone::clone).ok()?;
    context
        .state
        .transaction_bound
        .store(true, Ordering::Release);
    let event = context.event(OperLogStatus::Success, None);
    Some((event, AuditTransactionBinding { context }))
}

pub trait AuditOutboxPersistencePort: Send + Sync {
    fn record<'a>(
        &'a self,
        event: &'a AuditOperationEvent,
        max_attempts: i32,
    ) -> PersistenceFuture<'a, ()>;
}

/// 由 HTTP 边界使用的独立短事务 Outbox 写入器。
#[derive(Clone)]
pub struct AuditOutbox {
    persistence: Arc<dyn AuditOutboxPersistencePort>,
    max_attempts: i32,
    job_queue: Option<Arc<JobQueue>>,
}

impl AuditOutbox {
    pub fn new(
        persistence: Arc<dyn AuditOutboxPersistencePort>,
        default_max_attempts: i32,
    ) -> Self {
        Self {
            persistence,
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
        validate_audit_event(event)?;
        self.persistence.record(event, self.max_attempts).await?;
        if let Some(job_queue) = &self.job_queue {
            job_queue.notify_outbox().await;
        }
        Ok(())
    }
}

pub(crate) fn validate_audit_event(event: &AuditOperationEvent) -> AppResult<()> {
    validate_identifier("event_id", &event.event_id)?;
    validate_identifier("request_id", &event.request_id)?;
    crate::enforce_tenant_scope(&event.tenant_id)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn command() -> RecordOperLogCommand {
        RecordOperLogCommand {
            title: "更新用户".to_owned(),
            business_type: "update".to_owned(),
            method: "handler".to_owned(),
            request_method: "PUT".to_owned(),
            oper_name: "admin".to_owned(),
            oper_url: "/users/1".to_owned(),
            oper_ip: "127.0.0.1".to_owned(),
            oper_param: None,
            json_result: None,
            status: OperLogStatus::Failure,
            error_msg: Some("尚未完成".to_owned()),
            cost_time: 0,
        }
    }

    #[tokio::test]
    async fn transaction_binding_marks_attempt_and_commit_separately() {
        let context = AuditRequestContext {
            event_id: "event-1".to_owned(),
            request_id: "request-1".to_owned(),
            tenant_id: "tenant-a".to_owned(),
            base_command: command(),
            started_at: Instant::now(),
            state: Arc::new(AuditRequestState {
                transaction_bound: AtomicBool::new(false),
                transaction_committed: AtomicBool::new(false),
            }),
        };
        let observed = Arc::clone(&context.state);

        scope_audit_request(context, async {
            let (event, binding) = bind_current_audit().expect("审计上下文应存在");
            assert_eq!(event.event_id, "event-1");
            assert!(observed.transaction_bound.load(Ordering::Acquire));
            assert!(!observed.transaction_committed.load(Ordering::Acquire));
            binding.mark_committed();
        })
        .await;

        assert!(observed.transaction_committed.load(Ordering::Acquire));
    }
}
