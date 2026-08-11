use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use ryframe_db::EnqueueBackgroundJob;
use ryframe_kernel::{AppError, AppResult};
use serde::Serialize;

use crate::system::{EXPORT_CLEANUP_JOB_TYPE, MESSAGE_RETENTION_JOB_TYPE};

/// 调度目标允许的租户范围。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduledJobTargetScope {
    Tenant,
    System,
}

/// 生成安全任务载荷时由调度器提供的受控上下文。
#[derive(Clone, Debug)]
pub struct ScheduledJobContext<'a> {
    pub tenant_id: &'a str,
    pub schedule_id: i64,
    pub trigger_kind: &'a str,
    pub scheduled_for: DateTime<Utc>,
    pub max_runtime_seconds: i32,
    pub fire_key: &'a str,
}

/// 后端代码注册的调度目标，管理端不能提供函数名、命令、URL 或任意载荷。
pub trait ScheduledJobTarget: Send + Sync {
    fn handler_key(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    fn scope(&self) -> ScheduledJobTargetScope;
    fn job_type(&self) -> &'static str;
    fn available(&self) -> bool;
    fn priority(&self) -> i32;
    fn max_attempts(&self) -> i32;
    fn build_job(&self, context: &ScheduledJobContext<'_>) -> AppResult<EnqueueBackgroundJob>;
}

/// 调度目标公开描述，供管理端选择白名单项。
#[derive(Clone, Debug, Serialize)]
pub struct ScheduledJobTargetDescriptor {
    pub handler_key: String,
    pub display_name: String,
    pub scope: ScheduledJobTargetScope,
    pub job_type: String,
    pub available: bool,
}

/// 只接受代码内显式注册目标的调度白名单。
#[derive(Clone, Default)]
pub struct ScheduledJobTargetRegistry {
    targets: Arc<BTreeMap<String, Arc<dyn ScheduledJobTarget>>>,
}

impl ScheduledJobTargetRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_target(mut self, target: Arc<dyn ScheduledJobTarget>) -> AppResult<Self> {
        let handler_key = target.handler_key();
        if handler_key.is_empty() || handler_key.len() > 96 {
            return Err(AppError::Config(
                "调度目标 handler_key 必须为 1 到 96 字节".into(),
            ));
        }
        let targets = Arc::make_mut(&mut self.targets);
        if targets.insert(handler_key.to_owned(), target).is_some() {
            return Err(AppError::Config(format!("调度目标重复注册: {handler_key}")));
        }
        Ok(self)
    }

    pub fn get(&self, handler_key: &str) -> Option<Arc<dyn ScheduledJobTarget>> {
        self.targets.get(handler_key).cloned()
    }

    /// 返回当前配置下可用调度目标所需的后台任务类型。
    pub fn available_job_types(&self) -> Vec<&'static str> {
        self.targets
            .values()
            .filter(|target| target.available())
            .map(|target| target.job_type())
            .collect()
    }

    pub fn descriptors_for_tenant(&self, tenant_id: &str) -> Vec<ScheduledJobTargetDescriptor> {
        self.targets
            .values()
            .filter(|target| {
                target.scope() == ScheduledJobTargetScope::Tenant || tenant_id == "system"
            })
            .map(|target| ScheduledJobTargetDescriptor {
                handler_key: target.handler_key().to_owned(),
                display_name: target.display_name().to_owned(),
                scope: target.scope(),
                job_type: target.job_type().to_owned(),
                available: target.available(),
            })
            .collect()
    }

    pub fn built_in(message_center_enabled: bool) -> AppResult<Self> {
        Self::new()
            .with_target(Arc::new(ExportCleanupTarget))?
            .with_target(Arc::new(MessageRetentionTarget {
                available: message_center_enabled,
            }))
    }
}

struct ExportCleanupTarget;

impl ScheduledJobTarget for ExportCleanupTarget {
    fn handler_key(&self) -> &'static str {
        "system.export_result_cleanup"
    }

    fn display_name(&self) -> &'static str {
        "导出结果过期清理"
    }

    fn scope(&self) -> ScheduledJobTargetScope {
        ScheduledJobTargetScope::System
    }

    fn job_type(&self) -> &'static str {
        EXPORT_CLEANUP_JOB_TYPE
    }

    fn available(&self) -> bool {
        true
    }

    fn priority(&self) -> i32 {
        -10
    }

    fn max_attempts(&self) -> i32 {
        20
    }

    fn build_job(&self, context: &ScheduledJobContext<'_>) -> AppResult<EnqueueBackgroundJob> {
        build_system_cleanup_job(self, context)
    }
}

struct MessageRetentionTarget {
    available: bool,
}

impl ScheduledJobTarget for MessageRetentionTarget {
    fn handler_key(&self) -> &'static str {
        "system.message_retention_cleanup"
    }

    fn display_name(&self) -> &'static str {
        "消息过期清理"
    }

    fn scope(&self) -> ScheduledJobTargetScope {
        ScheduledJobTargetScope::System
    }

    fn job_type(&self) -> &'static str {
        MESSAGE_RETENTION_JOB_TYPE
    }

    fn available(&self) -> bool {
        self.available
    }

    fn priority(&self) -> i32 {
        -10
    }

    fn max_attempts(&self) -> i32 {
        20
    }

    fn build_job(&self, context: &ScheduledJobContext<'_>) -> AppResult<EnqueueBackgroundJob> {
        build_system_cleanup_job(self, context)
    }
}

fn build_system_cleanup_job(
    target: &dyn ScheduledJobTarget,
    context: &ScheduledJobContext<'_>,
) -> AppResult<EnqueueBackgroundJob> {
    if context.tenant_id != "system" || target.scope() != ScheduledJobTargetScope::System {
        return Err(AppError::Authorization(
            "当前租户不能运行平台维护调度目标".into(),
        ));
    }
    if !target.available() {
        return Err(AppError::ServiceUnavailable(
            "当前配置下调度目标不可用".into(),
        ));
    }
    let trace_context = crate::trace_context::current_trace_context();
    Ok(EnqueueBackgroundJob {
        tenant_id: None,
        schedule_id: Some(context.schedule_id),
        scheduled_for: Some(context.scheduled_for),
        max_runtime_seconds: Some(context.max_runtime_seconds),
        job_type: target.job_type().to_owned(),
        payload: serde_json::json!({
            "schedule_id": context.schedule_id.to_string(),
            "trigger_kind": context.trigger_kind,
            "scheduled_for": context.scheduled_for,
        }),
        priority: target.priority(),
        available_at: context.scheduled_for,
        max_attempts: target.max_attempts(),
        dedupe_key: Some(format!(
            "schedule:{}:{}",
            context.schedule_id, context.fire_key
        )),
        traceparent: trace_context.traceparent,
        tracestate: trace_context.tracestate,
    })
}
