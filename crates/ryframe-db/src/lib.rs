pub mod cluster;
pub mod connection;
pub mod data_scope;
pub mod database_monitor;
pub mod entities;
mod execution_tenant_scope;
pub mod pagination;
pub mod repositories;
pub mod sql_logger;
pub use cluster::{
    CallbackDatabaseMetricsObserver, ControlDatabaseCluster, DatabaseMetricsObserver,
    DatabaseNodeKind, DatabaseReadSelectionReason, ReadConsistency, SelectedDatabase,
};
pub use database_monitor::SeaOrmDatabaseMonitor;
pub use execution_tenant_scope::ExecutionTenantScope;
pub use sql_logger::{DbSpanLayer, SqlLogGuard, SqlLogLayer};
pub mod transaction;

#[doc(hidden)]
pub mod __macro_support {
    pub mod auto_fill {
        pub use ryframe_adapters::auto_fill::{AppResult, AutoFill, FillContext};

        pub fn next_id() -> AppResult<i64> {
            Ok(ryframe_utils::snowflake::try_next_snowflake_id()?)
        }
    }
}

// 便捷导出
pub use entities::{
    background_job, cache_namespace_version, config, data_retention_run, dept, dict_data,
    dict_type, export_job, job_schedule, job_schedule_execution, login_info, menu, message,
    message_audience, message_recipient, notice, oper_log, outbox_event, password_reset_request,
    permission, post, product_plan, product_plan_capability, product_plan_version, role, role_dept,
    role_permission, service_access_audit, service_account, service_account_role,
    service_credential, service_delegation, service_delegation_capability, sys_file, tenant,
    tenant_capability_override, tenant_config_bundle, tenant_config_transfer,
    tenant_config_transfer_item, tenant_data_backup_point, tenant_data_migration,
    tenant_data_migration_item, tenant_data_placement, tenant_operation_lease, tenant_product_plan,
    user, user_import_job, user_import_row_result, user_role,
};
pub use repositories::{
    AgentDictionaryPage, AgentQueryPage, AgentQueryRepository, AgentRowScope, BackgroundJobFilter,
    BackgroundJobRepository, BackgroundJobStats, BackgroundJobTypeStats, CONFIG_CACHE_NAMESPACE,
    CacheNamespaceVersionRepository, ConfigFilter, ConfigRepository, CreateExportJob,
    CreateTenantDataMigration, CreateUserImportJob, DataRetentionRepository, DeptRepository,
    DictDataRepository, DictTypeFilter, DictTypeRepository, EnqueueBackgroundJob,
    EnqueueBackgroundJobResult, ExpiredLeaseRecovery, ExportJobRepository, ExportQuerySnapshot,
    FailBackgroundJob, FileRepository, JobFailureDisposition, JobScheduleExecutionFilter,
    JobScheduleFilter, JobScheduleRepository, LoginInfoFilter, LoginInfoRepository,
    MarkExportJobSucceeded, MarkExportJobsDeletePending, MenuFilter, MenuRepository,
    MessageAudienceKind, MessageAudienceSelector, MessageInboxQuery, MessageRepository,
    NoticeFilter, NoticeRepository, OperLogFilter, OperLogRepository, OutboxEventRepository,
    OutboxFailureDisposition, OverviewRepository, OverviewTrendCount,
    PasswordResetRequestRepository, PermissionRepository, PostFilter, PostRepository,
    ProductPlanVersionBundle, ProductRepository, ProvisionTenantCommand, PublishMessageCommand,
    PublishedMessage, RecipientMessage, RecipientMessagePage, RecordOutboxEvent,
    RegisterTenantDataBackupPoint, RetentionCleanupResult, RetentionCutoff, RetentionResource,
    RoleFilter, RoleRepository, ScheduleOverviewStats, ServiceAccessAuditRepository,
    ServiceAccountLock, ServiceAccountRepository, ServiceAuthorizationRepository,
    ServiceAuthorizationSnapshot, ServiceCredentialRepository, ServiceDelegationRepository,
    TenantConfigTransferRepository, TenantConfigurationFence, TenantDataRepository,
    TenantOperationLeaseRepository, TenantProductBundle, TenantProvisioningRepository,
    TenantRepository, TenantUsageAggregate, TenantUsagePageFilter, TenantUsageRepository,
    UserFilter, UserImportArtifact, UserImportFilter, UserImportRepository, UserRepository,
    validate_cache_namespace,
};
