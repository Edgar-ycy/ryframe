pub mod agent;
mod audit;
mod auth_service;
mod authorization_cache;
mod authorization_resolver;
pub mod jobs;
mod service_identity_secret;
pub mod system;
mod trace_context;

pub use audit::{
    AUDIT_OPERATION_OUTBOX_EVENT_TYPE, AuditOperationEvent, AuditOutbox, AuditRequestContext,
    AuditTransactionBinding, commit_current_audit, record_audit_failure,
    record_current_audit_in_transaction, scope_audit_request, set_audit_failure_hook,
};
pub use auth_service::{AuthService, LoginResult, UserInfo};
pub use authorization_cache::{
    AUTHORIZATION_CHANGED_REDIS_CHANNEL, AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE,
    AUTHORIZATION_SNAPSHOT_TTL_SECS, AuthorizationCache, AuthorizationCacheBackend,
    AuthorizationCacheLookup, AuthorizationChangedEvent, AuthorizationMirrorUpdate,
    AuthorizationSnapshot, AuthorizationVersions, NamespaceCacheLookup, TenantCacheLookup,
    set_authorization_cache_lookup_hook,
};
pub(crate) use authorization_resolver::{AuthorizationResolver, ResolvedAuthorization};
#[allow(deprecated)]
pub use jobs::spawn_message_retention_scheduler;
pub use jobs::{
    BackgroundJobListParams, BackgroundJobQueueStats, BackgroundJobVo, CallbackJobMetricsObserver,
    CallbackScheduleMetricsObserver, CreateJobSchedule, ExportCleanupJobHandler, ExportJobHandler,
    JobHandler, JobMetricsObserver, JobQueue, JobRunResult, JobScheduleExecutionListParams,
    JobScheduleExecutionVo, JobScheduleListParams, JobScheduleOccurrence, JobSchedulePreview,
    JobScheduleService, JobScheduleVo, JobWorker, MessageDispatchJobHandler,
    MessageRetentionJobHandler, OutboxRunResult, OutboxWorker, ScheduleMetricsObserver,
    ScheduledJobContext, ScheduledJobTarget, ScheduledJobTargetDescriptor,
    ScheduledJobTargetRegistry, ScheduledJobTargetScope, UpdateJobSchedule,
};

use ryframe_kernel::{ActorContext, AppResult};

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    ryframe_core::validate_explicit_tenant(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}
