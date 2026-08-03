mod audit;
mod auth_service;
mod authorization_cache;
pub mod jobs;
pub mod system;
mod trace_context;

pub use audit::{
    AUDIT_OPERATION_OUTBOX_EVENT_TYPE, AuditOperationEvent, AuditOutbox, AuditRequestContext,
    AuditTransactionBinding, commit_current_audit, record_audit_failure,
    record_current_audit_in_transaction, scope_audit_request, set_audit_failure_hook,
};
pub use auth_service::{AuthService, LoginResult, UserInfo};
pub use authorization_cache::{
    AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE, AUTHORIZATION_SNAPSHOT_TTL_SECS, AuthorizationCache,
    AuthorizationCacheBackend, AuthorizationCacheLookup, AuthorizationMirrorUpdate,
    AuthorizationSnapshot, AuthorizationVersions, NamespaceCacheLookup, TenantCacheLookup,
};
pub use jobs::{
    BackgroundJobListParams, BackgroundJobQueueStats, BackgroundJobVo, CallbackJobMetricsObserver,
    ExportCleanupJobHandler, ExportJobHandler, JobHandler, JobMetricsObserver, JobQueue,
    JobRunResult, JobWorker, MessageDispatchJobHandler, MessageRetentionJobHandler,
    OutboxRunResult, OutboxWorker, spawn_message_retention_scheduler,
};

use ryframe_kernel::{ActorContext, AppResult};

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    ryframe_core::validate_explicit_tenant(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}
