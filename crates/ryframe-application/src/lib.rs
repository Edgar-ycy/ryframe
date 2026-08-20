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
mod principal_resolver;
mod refresh_session;
mod request_tenant_context;
mod runtime_policy;
mod service_identity_secret;
mod spreadsheet;
pub mod system;
mod tenant_runtime;
mod trace_context;

pub use artifact_store::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreErrorKind, ArtifactStoreFuture,
};
pub use audit::{
    AUDIT_OPERATION_OUTBOX_EVENT_TYPE, AuditOperationEvent, AuditOutbox, AuditRequestContext,
    AuditTransactionBinding, commit_current_audit, record_audit_failure,
    record_current_audit_in_transaction, scope_audit_request, set_audit_failure_hook,
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
pub use jobs::spawn_message_retention_scheduler;
pub use jobs::{
    BackgroundJobListParams, BackgroundJobQueueStats, BackgroundJobVo, CallbackJobMetricsObserver,
    CallbackScheduleMetricsObserver, CreateJobSchedule, ExportCleanupJobHandler, ExportJobHandler,
    JobHandler, JobMetricsObserver, JobQueue, JobRunResult, JobScheduleExecutionListParams,
    JobScheduleExecutionVo, JobScheduleListParams, JobScheduleOccurrence, JobSchedulePreview,
    JobScheduleService, JobScheduleVo, JobWakeupFuture, JobWakeupStream, JobWakeupTransport,
    JobWorker, MessageDispatchJobHandler, MessageRetentionJobHandler, MessageWakeupFuture,
    MessageWakeupPublisher, OutboxRunResult, OutboxWorker, ScheduleMetricsObserver,
    ScheduledJobContext, ScheduledJobTarget, ScheduledJobTargetDescriptor,
    ScheduledJobTargetRegistry, ScheduledJobTargetScope, UpdateJobSchedule,
};
pub use login_protection::{LoginProtectionFuture, LoginProtectionPort};
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
pub use tenant_runtime::{
    TenantBusinessDataState, TenantRuntimeReadFuture, TenantRuntimeReadPort, TenantRuntimeSnapshot,
};

use ryframe_kernel::{ActorContext, AppError, AppResult, TenantId};
use ryframe_tenant_db::TenantDataError;

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    enforce_tenant_scope(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}

pub(crate) fn enforce_tenant_scope(tenant_id: &str) -> AppResult<()> {
    request_tenant_context::enforce_tenant_context(TenantId::parse(tenant_id)?)
}

/// 在应用边界把租户数据基础设施错误映射为稳定领域错误，不依赖展示字符串。
pub fn map_tenant_data_error(error: TenantDataError) -> AppError {
    match error {
        TenantDataError::InvalidConfiguration(message) => AppError::Config(message),
        TenantDataError::InvalidTenantId(message) => AppError::Validation(message),
        TenantDataError::StalePlacementGeneration {
            tenant_id,
            session_generation,
            current_generation,
        } => AppError::StalePlacementGeneration(format!(
            "tenant={tenant_id}, session={session_generation}, current={current_generation}"
        )),
        TenantDataError::TenantDataMaintenance {
            tenant_id,
            generation,
        } => AppError::TenantDataMaintenance(
            format!("tenant={tenant_id}, generation={generation}"),
            5,
        ),
        TenantDataError::FenceRejected {
            tenant_id,
            target_key,
            ..
        } => AppError::TenantDataMaintenance(format!("tenant={tenant_id}, target={target_key}"), 5),
        TenantDataError::DedicatedTargetOccupied { target_key } => {
            AppError::TenantOperationConflict(format!(
                "dedicated tenant-data target is occupied: {target_key}"
            ))
        }
        TenantDataError::UnknownTarget { target_key }
        | TenantDataError::TargetUnavailable { target_key } => {
            AppError::TenantDataTargetUnavailable(
                format!("tenant-data target unavailable: {target_key}"),
                5,
            )
        }
        TenantDataError::PlacementUnavailable { tenant_id }
        | TenantDataError::InvalidPlacement { tenant_id, .. } => {
            AppError::TenantDataTargetUnavailable(
                format!("tenant-data placement unavailable: {tenant_id}"),
                5,
            )
        }
        TenantDataError::PoolCapacityExhausted { .. }
        | TenantDataError::ConnectionBudgetExhausted { .. } => {
            AppError::TenantDataTargetUnavailable("tenant-data pool capacity exhausted".into(), 5)
        }
    }
}
