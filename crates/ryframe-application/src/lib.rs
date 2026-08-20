pub mod agent;
mod artifact_store;
mod audit;
mod auth_service;
mod authorization_cache;
mod authorization_diagnostic_persistence;
mod authorization_mirror_persistence;
mod authorization_resolver;
mod config_persistence;
mod dept_persistence;
mod dict_persistence;
mod execution_tenant_scope;
mod file_content;
mod id_generator;
mod identity_authorization;
mod job_queue_persistence;
mod job_schedule_persistence;
pub mod jobs;
mod legacy_agent_audit;
mod legacy_agent_identity;
mod legacy_agent_persistence;
mod legacy_agent_snapshot;
mod legacy_audit_persistence;
mod legacy_authorization_diagnostic_persistence;
mod legacy_authorization_mirror_persistence;
mod legacy_config_persistence;
mod legacy_dept_persistence;
mod legacy_dict_persistence;
mod legacy_execution_tenant_scope;
mod legacy_identity_authorization;
mod legacy_job_queue_persistence;
mod legacy_job_schedule_persistence;
mod legacy_login_info_persistence;
mod legacy_menu_persistence;
mod legacy_message_persistence;
mod legacy_notice_persistence;
mod legacy_oper_log_persistence;
mod legacy_outbox_persistence;
mod legacy_overview_persistence;
mod legacy_password_reset_persistence;
mod legacy_permission_persistence;
mod legacy_post_persistence;
mod legacy_product_persistence;
mod legacy_profile_persistence;
mod legacy_retention_cleanup_persistence;
mod legacy_retention_run_persistence;
mod legacy_role_read;
mod legacy_role_write;
mod legacy_service_account_audit_persistence;
mod legacy_service_account_authorization_persistence;
mod legacy_service_account_read;
mod legacy_service_account_write;
mod legacy_tenant_config_retention_persistence;
mod legacy_tenant_usage_persistence;
mod legacy_user_import_persistence;
mod legacy_user_query_persistence;
mod legacy_user_write_persistence;
mod login_info_persistence;
mod login_protection;
mod menu_persistence;
mod message_persistence;
mod notice_persistence;
mod oper_log_persistence;
mod outbox_persistence;
mod overview_persistence;
mod password_reset_persistence;
mod permission_persistence;
mod persistence;
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
mod tenant_config_retention_persistence;
mod tenant_data_migration;
mod tenant_data_targets;
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
pub use authorization_diagnostic_persistence::{
    AuthorizationDiagnosticReadPort, DiagnosticDepartmentRecord, DiagnosticMenuRecord,
    DiagnosticPermissionRecord, DiagnosticRoleRecord,
};
pub use authorization_mirror_persistence::{
    AuthorizationMirrorEvent, AuthorizationMirrorTransaction,
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
pub use execution_tenant_scope::ExecutionTenantScope;
pub use file_content::{FileContentFuture, FileContentProcessor, ProcessedFileContent};
pub use id_generator::{BusinessIdGenerator, install as install_id_generator, next_id};
pub use identity_authorization::{
    IdentityAuthorizationReadPort, IdentityRoleRecord, IdentityTenantRecord, IdentityUserRecord,
};
pub use job_queue_persistence::{
    BackgroundJobPersistencePort, BackgroundJobReadFilter, BackgroundJobRecord,
    BackgroundJobStatsRecord, BackgroundJobTransaction, BackgroundJobTypeStats, ClaimedJobRecord,
    FailJobCommand, JobFailureOutcome, RecoveredJobLeases, TenantConfigJobKind,
};
pub use job_schedule_persistence::{
    JobScheduleExecutionReadFilter, JobScheduleExecutionRecord, JobSchedulePersistencePort,
    JobScheduleReadFilter, JobScheduleReadPort, JobScheduleRecord, JobScheduleTransaction,
    NewJobScheduleExecution,
};
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
#[doc(hidden)]
pub use legacy_agent_audit::port as legacy_agent_audit_write;
#[doc(hidden)]
pub use legacy_agent_identity::port as legacy_agent_identity_read;
#[doc(hidden)]
pub use legacy_agent_persistence::port as legacy_agent_persistence;
#[doc(hidden)]
pub use legacy_audit_persistence::{
    commit_current_audit, outbox_port as legacy_audit_outbox_persistence,
    record_current_audit_in_transaction,
};
#[doc(hidden)]
pub use legacy_authorization_diagnostic_persistence::port as legacy_authorization_diagnostic_persistence;
#[doc(hidden)]
pub use legacy_config_persistence::port as legacy_config_persistence;
#[doc(hidden)]
pub use legacy_dept_persistence::{read_port as legacy_dept_read, write_port as legacy_dept_write};
#[doc(hidden)]
pub use legacy_dict_persistence::port as legacy_dict_persistence;
#[doc(hidden)]
pub use legacy_identity_authorization::port as legacy_identity_authorization;
#[doc(hidden)]
pub use legacy_job_queue_persistence::port as legacy_job_queue_persistence;
#[doc(hidden)]
pub use legacy_job_schedule_persistence::port as legacy_job_schedule_persistence;
#[doc(hidden)]
pub use legacy_login_info_persistence::port as legacy_login_info_persistence;
#[doc(hidden)]
pub use legacy_menu_persistence::{read_port as legacy_menu_read, write_port as legacy_menu_write};
#[doc(hidden)]
pub use legacy_message_persistence::port as legacy_message_persistence;
#[doc(hidden)]
pub use legacy_notice_persistence::port as legacy_notice_persistence;
#[doc(hidden)]
pub use legacy_oper_log_persistence::port as legacy_oper_log_persistence;
#[doc(hidden)]
pub use legacy_outbox_persistence::port as legacy_outbox_persistence;
#[doc(hidden)]
pub use legacy_overview_persistence::port as legacy_overview_persistence;
#[doc(hidden)]
pub use legacy_password_reset_persistence::port as legacy_password_reset_persistence;
#[doc(hidden)]
pub use legacy_permission_persistence::{
    read_port as legacy_permission_read, write_port as legacy_permission_write,
};
#[doc(hidden)]
pub use legacy_post_persistence::port as legacy_post_persistence;
#[doc(hidden)]
pub use legacy_product_persistence::read_port as legacy_product_read;
#[doc(hidden)]
pub use legacy_profile_persistence::port as legacy_profile_persistence;
#[doc(hidden)]
pub use legacy_retention_cleanup_persistence::port as legacy_retention_cleanup_persistence;
#[doc(hidden)]
pub use legacy_retention_run_persistence::port as legacy_retention_run_persistence;
#[doc(hidden)]
pub use legacy_role_read::port as legacy_role_read;
#[doc(hidden)]
pub use legacy_role_write::port as legacy_role_write;
#[doc(hidden)]
pub use legacy_service_account_audit_persistence::port as legacy_service_account_audit_persistence;
#[doc(hidden)]
pub use legacy_service_account_authorization_persistence::port as legacy_service_account_authorization_persistence;
#[doc(hidden)]
pub use legacy_service_account_read::port as legacy_service_account_read;
#[doc(hidden)]
pub use legacy_service_account_write::port as legacy_service_account_write;
#[doc(hidden)]
pub use legacy_tenant_config_retention_persistence::port as legacy_tenant_config_retention_persistence;
#[doc(hidden)]
pub use legacy_tenant_usage_persistence::port as legacy_tenant_usage_persistence;
#[doc(hidden)]
pub use legacy_user_import_persistence::port as legacy_user_import_persistence;
#[doc(hidden)]
pub use legacy_user_query_persistence::port as legacy_user_query_persistence;
#[doc(hidden)]
pub use legacy_user_write_persistence::port as legacy_user_write_persistence;
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
pub use outbox_persistence::{ClaimedOutboxEvent, OutboxFailureOutcome, OutboxPersistencePort};
pub use overview_persistence::{
    OverviewPersistencePort, OverviewTrendCount, OverviewTrendSeries, ScheduleOverviewStats,
};
pub use password_reset_persistence::{
    NewPasswordResetRequest, PASSWORD_RESET_STATUS_PENDING, PasswordResetPersistencePort,
    PasswordResetRequestRecord, PasswordResetTransaction, PasswordResetUserState,
};
pub use permission_persistence::{
    PermissionReadPort, PermissionRecord, PermissionWritePort, PermissionWriteTransaction,
};
pub use persistence::{ControlTransaction, PersistenceFuture};
pub use post_persistence::{PostFilter, PostPersistencePort, PostRecord, PostTransaction};
pub use principal_resolver::PrincipalResolver;
pub use product_persistence::{
    ProductCapabilityRecord, ProductPlanRecord, ProductReadPort, ProductVersionRecord,
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
pub use tenant_config_retention_persistence::{
    TENANT_CONFIG_PACKAGE_RESOURCE, TENANT_CONFIG_SNAPSHOT_RESOURCE, TenantConfigArtifactCounts,
    TenantConfigRetentionPersistencePort,
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
pub use tenant_usage_persistence::{
    TenantCapacityRecord, TenantUsageAggregateRecord, TenantUsageFilter, TenantUsagePersistencePort,
};
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
