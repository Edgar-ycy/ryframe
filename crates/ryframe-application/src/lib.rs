pub mod agent;
mod artifact_store;
mod audit;
mod auth_service;
mod authorization_cache;
mod authorization_resolver;
mod file_content;
mod id_generator;
pub mod jobs;
mod login_protection;
mod persistence;
pub mod ports;
mod principal_resolver;
mod refresh_session;
mod request_tenant_context;
mod runtime_policy;
mod service_identity_secret;
mod spreadsheet;
pub mod system;
mod tenant_config_archive;
#[doc(hidden)]
pub mod tenant_config_stable_key;
mod trace_context;

pub use artifact_store::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreErrorKind, ArtifactStoreFuture,
};
#[doc(hidden)]
pub use audit::{
    AUDIT_AGGREGATE_TYPE, OUTBOX_MAX_ATTEMPTS, bind_current_audit, validate_audit_event,
};
pub use audit::{
    AUDIT_OPERATION_OUTBOX_EVENT_TYPE, AuditOperationEvent, AuditOutbox,
    AuditOutboxPersistencePort, AuditRequestContext, AuditTransactionBinding, record_audit_failure,
    scope_audit_request, set_audit_failure_hook,
};
pub use auth_service::{AuthService, LoginResult, UserInfo};
pub use authorization_cache::{
    AUTHORIZATION_CHANGED_REDIS_CHANNEL, AUTHORIZATION_MIRROR_OUTBOX_EVENT_TYPE,
    AUTHORIZATION_SNAPSHOT_TTL_SECS, AuthorizationCache, AuthorizationCacheBackend,
    AuthorizationCacheLookup, AuthorizationChangePublishFuture, AuthorizationChangePublisher,
    AuthorizationChangedEvent, AuthorizationMirrorUpdate, AuthorizationSnapshot,
    AuthorizationVersions, NamespaceCacheLookup, TenantCacheLookup,
    set_authorization_cache_lookup_hook,
};
pub(crate) use authorization_resolver::{AuthorizationResolver, ResolvedAuthorization};
pub use file_content::{FileContentFuture, FileContentProcessor, ProcessedFileContent};
pub use id_generator::{BusinessIdGenerator, install as install_id_generator, next_id};
#[allow(deprecated)]
pub use jobs::{
    BackgroundJobListParams, BackgroundJobQueueStats, BackgroundJobVo, CallbackJobMetricsObserver,
    CallbackScheduleMetricsObserver, ClaimedBackgroundJob, CreateJobSchedule, EnqueueJob,
    EnqueueJobResult, ExportCleanupJobHandler, ExportJobHandler, JobHandler, JobMetricsObserver,
    JobQueue, JobRunResult, JobScheduleExecutionListParams, JobScheduleExecutionVo,
    JobScheduleListParams, JobScheduleOccurrence, JobSchedulePreview, JobScheduleService,
    JobScheduleVo, JobWakeupFuture, JobWakeupStream, JobWakeupTransport, JobWorker,
    MessageDispatchJobHandler, MessageRetentionJobHandler, MessageWakeupFuture,
    MessageWakeupPublisher, OutboxRunResult, OutboxWorker, ScheduleMetricsObserver,
    ScheduledJobContext, ScheduledJobTarget, ScheduledJobTargetDescriptor,
    ScheduledJobTargetRegistry, ScheduledJobTargetScope, UpdateJobSchedule,
};
pub use login_protection::{LoginProtectionFuture, LoginProtectionPort};
pub use persistence::{ControlTransaction, PersistenceFuture};
pub use principal_resolver::PrincipalResolver;
pub use refresh_session::{
    RefreshSessionFamily, RefreshSessionFuture, RefreshSessionIdentity, RefreshSessionPort,
    RefreshSessionRevocation, RefreshSessionRotation,
};
pub use request_tenant_context::{TenantContext, with_tenant_context};
pub use runtime_policy::{
    AuthPolicy, CacheAvailabilityPolicy, ExportPolicy, JobRuntimePolicy, JobSchedulePolicy,
    JobWorkerMode, JobWorkerPolicy, MessagingPolicy, MultiTenancyPolicy, PepperKeyring,
    ServiceAccountPolicy, TenantConfigTransferPolicy, UserImportPolicy,
};
pub use spreadsheet::{
    SPREADSHEET_MAX_DATA_ROWS, SpreadsheetArtifact, SpreadsheetBatchProgress,
    SpreadsheetDocumentFuture, SpreadsheetDocumentProcessor, SpreadsheetImportRow, SpreadsheetRow,
    SpreadsheetWriter, SpreadsheetWriterFactory,
};
pub use tenant_config_archive::{TenantConfigArchiveContents, TenantConfigArchivePort};
#[doc(hidden)]
pub use trace_context::current_trace_context;
pub use trace_context::{PersistedTraceContext, TraceContextPort, install_trace_context_port};

use ryframe_kernel::{ActorContext, AppResult, TenantId};

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    enforce_tenant_scope(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}

pub(crate) fn enforce_tenant_scope(tenant_id: &str) -> AppResult<()> {
    request_tenant_context::enforce_tenant_context(TenantId::parse(tenant_id)?)
}
