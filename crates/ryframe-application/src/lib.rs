pub mod agent;
mod artifact_store;
mod audit;
mod auth_service;
mod authorization_cache;
mod authorization_resolver;
mod config_persistence;
mod dict_persistence;
mod file_content;
mod id_generator;
pub mod jobs;
mod legacy_config_persistence;
mod legacy_dict_persistence;
mod legacy_login_info_persistence;
mod legacy_notice_persistence;
mod legacy_oper_log_persistence;
mod legacy_overview_persistence;
mod legacy_post_persistence;
mod legacy_profile_persistence;
mod legacy_role_read;
mod legacy_role_write;
mod login_info_persistence;
mod login_protection;
mod notice_persistence;
mod oper_log_persistence;
mod overview_persistence;
mod persistence;
mod post_persistence;
mod principal_resolver;
mod profile_persistence;
mod refresh_session;
mod request_tenant_context;
mod role_read;
mod role_write;
mod runtime_policy;
mod service_identity_secret;
mod spreadsheet;
pub mod system;
mod tenant_data_migration;
mod tenant_data_targets;
mod tenant_provisioning;
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
pub use config_persistence::{
    ConfigFilter, ConfigPersistencePort, ConfigRecord, ConfigTransaction,
};
pub use dict_persistence::{
    DictDataRecord, DictPersistencePort, DictTransaction, DictTypeFilter, DictTypeRecord,
};
pub use file_content::{FileContentFuture, FileContentProcessor, ProcessedFileContent};
pub use id_generator::{BusinessIdGenerator, install as install_id_generator, next_id};
#[allow(deprecated)]
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
#[doc(hidden)]
pub use legacy_config_persistence::port as legacy_config_persistence;
#[doc(hidden)]
pub use legacy_dict_persistence::port as legacy_dict_persistence;
#[doc(hidden)]
pub use legacy_login_info_persistence::port as legacy_login_info_persistence;
#[doc(hidden)]
pub use legacy_notice_persistence::port as legacy_notice_persistence;
#[doc(hidden)]
pub use legacy_oper_log_persistence::port as legacy_oper_log_persistence;
#[doc(hidden)]
pub use legacy_overview_persistence::port as legacy_overview_persistence;
#[doc(hidden)]
pub use legacy_post_persistence::port as legacy_post_persistence;
#[doc(hidden)]
pub use legacy_profile_persistence::port as legacy_profile_persistence;
#[doc(hidden)]
pub use legacy_role_read::port as legacy_role_read;
#[doc(hidden)]
pub use legacy_role_write::port as legacy_role_write;
pub use login_info_persistence::{
    LoginInfoFilter, LoginInfoPersistencePort, LoginInfoRecord, LoginInfoTransaction,
};
pub use login_protection::{LoginProtectionFuture, LoginProtectionPort};
pub use notice_persistence::{
    NoticeFilter, NoticePersistencePort, NoticeRecord, NoticeTransaction,
};
pub use oper_log_persistence::{
    OperLogFilter, OperLogPersistencePort, OperLogRecord, OperLogTransaction,
};
pub use overview_persistence::{
    OverviewPersistencePort, OverviewTrendCount, OverviewTrendSeries, ScheduleOverviewStats,
};
pub use persistence::{ControlTransaction, PersistenceFuture};
pub use post_persistence::{PostFilter, PostPersistencePort, PostRecord, PostTransaction};
pub use principal_resolver::PrincipalResolver;
pub use profile_persistence::{
    ProfileAvatarFile, ProfileAvatarState, ProfilePersistencePort, ProfileRecord,
    ProfileTransaction, ProfileUserState,
};
pub use refresh_session::{
    RefreshSessionFamily, RefreshSessionFuture, RefreshSessionIdentity, RefreshSessionPort,
    RefreshSessionRevocation, RefreshSessionRotation,
};
pub use request_tenant_context::{TenantContext, with_tenant_context};
pub use role_read::{RoleFilter, RoleReadPort, RoleRecord};
pub use role_write::{RolePermissionRef, RoleWritePort, RoleWriteTransaction};
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
pub use tenant_data_migration::{
    TenantDataCatalogTable, TenantDataCleanupOwnership, TenantDataFence, TenantDataMigrationFuture,
    TenantDataMigrationPort, TenantDataRow, TenantDataRowBatch,
};
pub use tenant_data_targets::{
    TenantDataPoolStats, TenantDataTargetAccess, TenantDataTargetFuture, TenantDataTargetHealth,
    TenantDataTargetMetadata, TenantDataTargetPort,
};
pub use tenant_provisioning::{
    TenantProvisioningFuture, TenantProvisioningPlacement, TenantProvisioningPort,
};
pub use tenant_runtime::{
    TenantBusinessDataState, TenantRuntimeReadFuture, TenantRuntimeReadPort, TenantRuntimeSnapshot,
};

use ryframe_kernel::{ActorContext, AppResult, TenantId};

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    enforce_tenant_scope(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}

pub(crate) fn enforce_tenant_scope(tenant_id: &str) -> AppResult<()> {
    request_tenant_context::enforce_tenant_context(TenantId::parse(tenant_id)?)
}
