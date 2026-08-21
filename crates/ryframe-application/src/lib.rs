pub mod agent;
mod artifact_store;
mod audit;
mod auth_service;
mod authorization_cache;
mod authorization_resolver;
mod config_persistence;
mod dept_persistence;
mod dict_persistence;
mod file_content;
mod id_generator;
pub mod jobs;
mod login_info_persistence;
mod login_protection;
mod menu_persistence;
mod message_persistence;
mod notice_persistence;
mod oper_log_persistence;
mod overview_persistence;
mod permission_persistence;
mod persistence;
pub mod ports;
mod post_persistence;
mod principal_resolver;
mod product_persistence;
mod profile_persistence;
mod refresh_session;
mod request_tenant_context;
mod retention_cleanup_persistence;
mod retention_run_persistence;
mod role_read;
mod role_write;
mod runtime_policy;
mod service_account_audit_persistence;
mod service_account_authorization_persistence;
mod service_account_read;
mod service_account_write;
mod service_identity_secret;
mod spreadsheet;
pub mod system;
mod tenant_config_archive;
mod tenant_config_retention_persistence;
#[doc(hidden)]
pub mod tenant_config_stable_key;
mod tenant_config_transfer_persistence;
mod tenant_data_migration;
mod tenant_data_migration_persistence;
mod tenant_data_targets;
mod tenant_persistence;
mod tenant_provisioning;
mod tenant_runtime;
mod tenant_usage_persistence;
mod trace_context;
mod user_import_persistence;
mod user_query_persistence;
mod user_write_persistence;

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
pub use config_persistence::{
    ConfigFilter, ConfigPersistencePort, ConfigRecord, ConfigTransaction,
};
pub use dept_persistence::{
    DeptFilter, DeptReadPort, DeptRecord, DeptTreeRecord, DeptWritePort, DeptWriteTransaction,
};
pub use dict_persistence::{
    DictDataRecord, DictPersistencePort, DictTransaction, DictTypeFilter, DictTypeRecord,
};
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
pub use login_info_persistence::{
    LoginInfoFilter, LoginInfoPersistencePort, LoginInfoRecord, LoginInfoTransaction,
};
pub use login_protection::{LoginProtectionFuture, LoginProtectionPort};
pub use menu_persistence::{
    MenuFilter, MenuReadPort, MenuRecord, MenuTreeRecord, MenuWritePort, MenuWriteTransaction,
};
pub use message_persistence::{
    MessageAudienceRecord, MessageAudienceRecordKind, MessageInboxFilter, MessageOutboxRecord,
    MessagePage, MessagePersistencePort, MessageRecipientRecord, MessageRecord, MessageTransaction,
    PublishMessageRecord, PublishedMessageRecord,
};
pub use notice_persistence::{
    NoticeFilter, NoticePersistencePort, NoticeRecord, NoticeTransaction,
};
pub use oper_log_persistence::{
    OperLogFilter, OperLogPersistencePort, OperLogRecord, OperLogTransaction,
};
pub use overview_persistence::{
    OverviewPersistencePort, OverviewTrendCount, OverviewTrendSeries, ScheduleOverviewStats,
};
pub use permission_persistence::{
    PermissionReadPort, PermissionRecord, PermissionWritePort, PermissionWriteTransaction,
};
pub use persistence::{ControlTransaction, PersistenceFuture};
pub use post_persistence::{PostFilter, PostPersistencePort, PostRecord, PostTransaction};
pub use principal_resolver::PrincipalResolver;
pub use product_persistence::{
    ProductAssignmentChange, ProductCapabilityRecord, ProductChangeTenantState, ProductPlanRecord,
    ProductPlanState, ProductReadPort, ProductTransactionPort, ProductVersionRecord,
    ProductVersionSnapshot, ProductVersionState, ProductVersionWriteResult, ProductWritePort,
    ProductWriteTransaction, ProvisioningCapabilityResources, TenantCapabilityOverrideRecord,
    TenantProductSnapshot,
};
pub use profile_persistence::{
    ProfileAvatarFile, ProfileAvatarState, ProfilePersistencePort, ProfileRecord,
    ProfileTransaction, ProfileUserState,
};
pub use refresh_session::{
    RefreshSessionFamily, RefreshSessionFuture, RefreshSessionIdentity, RefreshSessionPort,
    RefreshSessionRevocation, RefreshSessionRotation,
};
pub use request_tenant_context::{TenantContext, with_tenant_context};
pub use retention_cleanup_persistence::{
    ExpiredImportArtifact, RetentionCleanupPersistencePort, RetentionCleanupResult,
    RetentionCutoff, RetentionResource,
};
pub use retention_run_persistence::{
    RetentionRunPersistencePort, RetentionRunRecord, RetentionRunTransaction,
};
pub use role_read::{RoleFilter, RoleReadPort, RoleRecord};
pub use role_write::{RolePermissionRef, RoleWritePort, RoleWriteTransaction};
pub use runtime_policy::{
    AuthPolicy, CacheAvailabilityPolicy, ExportPolicy, JobRuntimePolicy, JobSchedulePolicy,
    JobWorkerMode, JobWorkerPolicy, MessagingPolicy, MultiTenancyPolicy, PepperKeyring,
    ServiceAccountPolicy, TenantConfigTransferPolicy, UserImportPolicy,
};
pub use service_account_audit_persistence::{
    ServiceAccessAuditRecord, ServiceAccountAuditReadPort,
};
pub use service_account_authorization_persistence::{
    ServiceAccountAuthorizationReadPort, ServiceAccountPermissionSnapshot,
    ServiceDelegationTargetRecord, ServiceDelegationTargetSet,
};
pub use service_account_read::{
    ServiceAccountDetailRecord, ServiceAccountReadPort, ServiceAccountRecord,
    ServiceCredentialRecord, ServiceDelegationRecord,
};
pub use service_account_write::{
    ServiceAccountUserRecord, ServiceAccountWritePort, ServiceAccountWriteTransaction,
    ServiceCredentialWriteRecord, ServiceDelegationIdentity, ServiceDelegationWriteRecord,
};
pub use spreadsheet::{
    SPREADSHEET_MAX_DATA_ROWS, SpreadsheetArtifact, SpreadsheetBatchProgress,
    SpreadsheetDocumentFuture, SpreadsheetDocumentProcessor, SpreadsheetImportRow, SpreadsheetRow,
    SpreadsheetWriter, SpreadsheetWriterFactory,
};
pub use tenant_config_archive::{TenantConfigArchiveContents, TenantConfigArchivePort};
pub use tenant_config_retention_persistence::{
    TENANT_CONFIG_PACKAGE_RESOURCE, TENANT_CONFIG_SNAPSHOT_RESOURCE, TenantConfigArtifactCounts,
    TenantConfigRetentionPersistencePort,
};
pub use tenant_config_transfer_persistence::{
    TenantConfigBundleRecord, TenantConfigOperationLeaseRecord, TenantConfigRequesterRecord,
    TenantConfigTransferItemRecord, TenantConfigTransferPersistencePort,
    TenantConfigTransferRecord, TenantConfigTransferTransaction, TenantConfigurationFenceRecord,
};
pub use tenant_data_migration::{
    TenantDataCatalogTable, TenantDataCleanupOwnership, TenantDataFence, TenantDataMigrationFuture,
    TenantDataMigrationPort, TenantDataRow, TenantDataRowBatch,
};
pub use tenant_data_migration_persistence::{
    CreateTenantDataMigrationRecord, MIGRATION_ITEM_CLEANUP_CLEANED,
    MIGRATION_ITEM_CLEANUP_CLEANING, MIGRATION_ITEM_CLEANUP_PENDING, MIGRATION_ITEM_STATE_COPIED,
    MIGRATION_ITEM_STATE_COPYING, MIGRATION_ITEM_STATE_PENDING, MIGRATION_ITEM_STATE_VERIFIED,
    MIGRATION_ITEM_STATE_VERIFYING, MIGRATION_STATE_ACTIVATING, MIGRATION_STATE_CANCELLED,
    MIGRATION_STATE_COPYING, MIGRATION_STATE_CUTTING_OVER, MIGRATION_STATE_FAILED,
    MIGRATION_STATE_FINALIZED, MIGRATION_STATE_FROZEN, MIGRATION_STATE_PRECHECKING,
    MIGRATION_STATE_QUEUED, MIGRATION_STATE_QUIESCING, MIGRATION_STATE_RETENTION_PENDING,
    MIGRATION_STATE_SUCCEEDED, MIGRATION_STATE_VERIFYING, PLACEMENT_STATE_ACTIVE,
    PLACEMENT_STATE_MAINTENANCE, TenantDataBackupPointRecord, TenantDataMigrationItemRecord,
    TenantDataMigrationPersistencePort, TenantDataMigrationRecord, TenantDataMigrationTransaction,
    TenantDataPlacementRecord, TenantMigrationContextRecord, TenantOperationLeaseRecord,
};
pub use tenant_data_targets::{
    TenantDataPoolStats, TenantDataTargetAccess, TenantDataTargetFuture, TenantDataTargetHealth,
    TenantDataTargetMetadata, TenantDataTargetPort,
};
pub use tenant_persistence::{
    ProvisionTenantRecord, TENANT_STATUS_DISABLED, TENANT_STATUS_ENABLED,
    TENANT_STATUS_PROVISIONING, TENANT_STATUS_PROVISIONING_FAILED, TenantAdminRecord,
    TenantPersistencePort, TenantProductAssignmentRecord, TenantProvisionRequestRecord,
    TenantRecord, TenantTransaction,
};
pub use tenant_provisioning::{
    TenantProvisioningFuture, TenantProvisioningPlacement, TenantProvisioningPort,
};
pub use tenant_runtime::{
    TenantBusinessDataState, TenantRuntimeReadFuture, TenantRuntimeReadPort, TenantRuntimeSnapshot,
};
pub use tenant_usage_persistence::{
    TenantCapacityRecord, TenantUsageAggregateRecord, TenantUsageFilter, TenantUsagePersistencePort,
};
#[doc(hidden)]
pub use trace_context::current_trace_context;
pub use trace_context::{PersistedTraceContext, TraceContextPort, install_trace_context_port};
pub use user_import_persistence::{
    NewImportedUser, NewUserImportJob, NewUserImportRow, UserImportAuthorizationSnapshot,
    UserImportDepartmentRecord, UserImportJobRecord, UserImportPersistencePort,
    UserImportReadFilter, UserImportRowRecord, UserImportSourceRecord, UserImportSourceState,
    UserImportTransaction,
};
pub use user_query_persistence::{
    USER_QUERY_STATUS_NORMAL, UserQueryDetailRecord, UserQueryFilter, UserQueryReadPort,
    UserQueryRecord, UserQueryRoleRecord,
};
pub use user_write_persistence::{
    ManageableUserState, NewUserRecord, USER_STATUS_DISABLED, USER_STATUS_MUST_RESET_PASSWORD,
    USER_STATUS_NORMAL, USER_STATUS_PENDING_ACTIVATION, UpdateUserRecord, UserAssignmentRole,
    UserAssignmentState, UserWritePersistencePort, UserWriteRecord, UserWriteTransaction,
};

use ryframe_kernel::{ActorContext, AppResult, TenantId};

pub(crate) fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {
    enforce_tenant_scope(&actor.tenant_id)?;
    Ok(&actor.tenant_id)
}

pub(crate) fn enforce_tenant_scope(tenant_id: &str) -> AppResult<()> {
    request_tenant_context::enforce_tenant_context(TenantId::parse(tenant_id)?)
}
