mod auth_service;
pub mod jobs;
pub mod system;
mod trace_context;

pub use auth_service::{AuthService, LoginResult, UserInfo};
pub use jobs::{
    BackgroundJobListParams, BackgroundJobQueueStats, BackgroundJobVo, CallbackJobMetricsObserver,
    ExportCleanupJobHandler, ExportJobHandler, JobHandler, JobMetricsObserver, JobQueue,
    JobRunResult, JobWorker, MessageDispatchJobHandler, MessageRetentionJobHandler,
    OPER_LOG_JOB_TYPE, OperLogJobHandler, OutboxRunResult, OutboxWorker,
    spawn_message_retention_scheduler,
};

use ryframe_kernel::{ActorContext, AppResult};

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    ryframe_core::validate_explicit_tenant(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}
