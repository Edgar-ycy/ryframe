mod audit;
mod auth;
mod authorization_cache;
mod authorization_resolver;
pub mod generated;
mod id_generator;
pub mod jobs;
mod persistence;
pub mod ports;
mod principal_resolver;
mod request_tenant_context;
mod runtime_policy;
pub mod system;
mod tenant_config_stable_key;
mod trace_context;

/// 数据库实现与组合根所需的窄适配面，不属于业务用例公开 API。
#[doc(hidden)]
pub mod infrastructure {
    pub use crate::audit::{
        AUDIT_AGGREGATE_TYPE, OUTBOX_MAX_ATTEMPTS, bind_current_audit, validate_audit_event,
    };
    pub use crate::tenant_config_stable_key::*;
    pub use crate::trace_context::current_trace_context;
}
pub use audit::{
    AUDIT_OPERATION_OUTBOX_EVENT_TYPE, AuditOperationEvent, AuditOutbox,
    AuditOutboxPersistencePort, AuditRequestContext, AuditTransactionBinding, record_audit_failure,
    scope_audit_request, set_audit_failure_hook,
};
pub use auth::{AuthService, LoginResult, UserInfo};
pub use authorization_cache::{
    AUTHORIZATION_CHANGED_REDIS_CHANNEL, AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE,
    AUTHORIZATION_SNAPSHOT_TTL_SECS, AuthorizationCache, AuthorizationCacheBackend,
    AuthorizationCacheLookup, AuthorizationChangePublisher, AuthorizationChangedEvent,
    AuthorizationMirrorUpdate, AuthorizationSnapshot, AuthorizationVersions, NamespaceCacheLookup,
    TenantCacheLookup, set_authorization_cache_lookup_hook, validate_cache_namespace,
};
pub use authorization_resolver::has_super_admin_role;
pub(crate) use authorization_resolver::{AuthorizationResolver, ResolvedAuthorization};
pub use id_generator::{BusinessIdGenerator, install as install_id_generator, next_id};
pub use jobs::{
    BackgroundJobListParams, BackgroundJobQueueStats, BackgroundJobVo, CallbackJobMetricsObserver,
    CallbackScheduleMetricsObserver, ClaimedBackgroundJob, CreateJobSchedule, EnqueueJob,
    EnqueueJobResult, ExportCleanupJobHandler, ExportJobHandler, JobHandler, JobMetricsObserver,
    JobQueue, JobRunResult, JobScheduleExecutionListParams, JobScheduleExecutionVo,
    JobScheduleListParams, JobScheduleOccurrence, JobSchedulePreview, JobScheduleService,
    JobScheduleVo, JobWakeupStream, JobWakeupTransport, JobWorker, MessageDispatchJobHandler,
    MessageRetentionJobHandler, MessageWakeupPublisher, OutboxRunResult, OutboxWorker,
    ScheduleMetricsObserver, ScheduledJobContext, ScheduledJobTarget, ScheduledJobTargetDescriptor,
    ScheduledJobTargetRegistry, ScheduledJobTargetScope, UpdateJobSchedule,
    maintenance_schedule_targets, message_schedule_targets,
    validate_persisted_schedule_configuration,
};
pub use persistence::{PersistenceTransaction, TransactionAuditMode, complete_transaction};
pub use principal_resolver::PrincipalResolver;
pub use request_tenant_context::{TenantContext, with_tenant_context};
pub use runtime_policy::{
    AuthPolicy, CacheAvailabilityPolicy, ExportPolicy, JobRuntimePolicy, JobSchedulePolicy,
    JobWorkerMode, JobWorkerPolicy, MessagingPolicy, MultiTenancyPolicy,
    TenantConfigTransferPolicy, UserImportPolicy, is_valid_tenant_target_key,
};
pub use trace_context::{
    HTTP_REQUEST_LOG_SPAN_TARGET, PersistedTraceContext, TraceContextPort,
    install_trace_context_port,
};

use ryframe_kernel::{ActorContext, AppResult, TenantId};

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    enforce_tenant_scope(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}

pub(crate) fn enforce_tenant_scope(tenant_id: &str) -> AppResult<()> {
    request_tenant_context::enforce_tenant_context(TenantId::parse(tenant_id)?)
}
