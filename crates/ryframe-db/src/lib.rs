#[cfg(feature = "repositories")]
mod auto_fill;
#[cfg(feature = "repositories")]
pub mod cluster;
#[cfg(feature = "migration")]
pub mod connection;
#[cfg(feature = "repositories")]
pub mod data_scope;
#[cfg(feature = "repositories")]
pub mod database_monitor;
#[cfg(feature = "migration")]
mod db_result;
#[cfg(feature = "repositories")]
pub mod entities;
#[cfg(feature = "migration")]
pub mod generated;
#[cfg(feature = "repositories")]
mod id_generator;
#[cfg(feature = "migration")]
pub mod migration;
#[cfg(feature = "repositories")]
pub mod pagination;
#[cfg(feature = "repositories")]
pub mod repositories;
#[cfg(feature = "repositories")]
pub mod repository;
#[cfg(feature = "repositories")]
pub mod resource_ownership;
#[cfg(feature = "repositories")]
pub mod sql_logger;
#[cfg(feature = "repositories")]
pub use auto_fill::{AutoFill, FillContext};
#[cfg(feature = "repositories")]
pub use cluster::{
    CallbackDatabaseMetricsObserver, ControlDatabaseCluster, DatabaseMetricsObserver,
    DatabaseNodeHealth, DatabaseNodeKind, DatabaseReadSelectionReason, DatabaseTopologyHealth,
    ReadConsistency, SelectedDatabase,
};
#[cfg(feature = "repositories")]
pub use database_monitor::SeaOrmDatabaseMonitor;
#[cfg(feature = "migration")]
pub use db_result::DbResultExt;
#[cfg(feature = "repositories")]
pub use id_generator::{DatabaseIdGenerator, install as install_id_generator, next_id};
#[cfg(feature = "repositories")]
pub(crate) use repository::Repository;
#[cfg(feature = "telemetry")]
pub use sql_logger::DbSpanLayer;
#[cfg(feature = "repositories")]
pub use sql_logger::{SqlLogGuard, SqlLogLayer};
#[cfg(feature = "repositories")]
pub mod transaction;

#[cfg(feature = "repositories")]
#[doc(hidden)]
pub mod __macro_support {
    pub mod auto_fill {
        pub use crate::auto_fill::{AutoFill, FillContext};
        pub use ryframe_kernel::AppResult;

        pub fn next_id() -> AppResult<i64> {
            crate::next_id()
        }
    }
}

// 仅供数据库 crate 内部保持简洁；跨 crate 调用必须经 entities/repositories 模块显式表达边界。
#[allow(unused_imports)]
#[cfg(feature = "repositories")]
pub(crate) use entities::{
    background_job, cache_namespace_version, config, data_retention_run, dept, dict_data,
    dict_type, export_job, job_schedule, job_schedule_execution, login_info, menu, message,
    message_audience, message_recipient, oper_log, outbox_event, password_reset_request,
    permission, product_plan, product_plan_capability, product_plan_version, role, role_dept,
    role_permission, service_access_audit, service_account, service_account_role,
    service_credential, service_delegation, service_delegation_capability, sys_file, tenant, user,
    user_import_job, user_import_row_result, user_role,
};
#[allow(unused_imports)]
#[cfg(feature = "repositories")]
pub(crate) use repositories::{
    AgentDictionaryPage, AgentQueryPage, AgentQueryRepository, AgentRowScope, BackgroundJobFilter,
    BackgroundJobRepository, BackgroundJobStats, BackgroundJobTypeStats, CONFIG_CACHE_NAMESPACE,
    CacheNamespaceVersionRepository, ConfigFilter, ConfigRepository, CreateExportJob,
    CreateTenantDataMigration, CreateUserImportJob, DataRetentionRepository, DeptRepository,
    DictDataRepository, DictTypeFilter, DictTypeRepository, EnqueueBackgroundJob,
    EnqueueBackgroundJobResult, ExpiredLeaseRecovery, ExportJobRepository, ExportStartDisposition,
    FailBackgroundJob, FileRepository, JobFailureDisposition, JobScheduleExecutionFilter,
    JobScheduleFilter, JobScheduleRepository, LoginInfoFilter, LoginInfoRepository,
    MarkExportJobSucceeded, MarkExportJobsDeletePending, MenuFilter, MenuRepository,
    MessageAudienceKind, MessageAudienceSelector, MessageInboxQuery, MessageRepository,
    OperLogFilter, OperLogRepository, OutboxEventRepository, OutboxFailureDisposition,
    OverviewRepository, OverviewTrendCount, PasswordResetRequestRepository, PermissionRepository,
    PostExportFilter, PostExportRepository, ProductPlanVersionBundle, ProductRepository,
    PublishMessageCommand, PublishedMessage, RecipientMessage, RecipientMessagePage,
    RecordOutboxEvent, RegisterTenantDataBackupPoint, RetentionCleanupResult, RetentionCutoff,
    RetentionResource, RoleFilter, RoleRepository, ScheduleOverviewStats,
    ServiceAccessAuditRepository, ServiceAccountLock, ServiceAccountRepository,
    ServiceAuthorizationRepository, ServiceAuthorizationSnapshot, ServiceCredentialRepository,
    ServiceDelegationRepository, TenantConfigTransferRepository, TenantConfigurationFence,
    TenantDataRepository, TenantOperationLeaseRepository, TenantProductBundle,
    TenantProvisioningRepository, TenantRepository, TenantUsageAggregate, TenantUsagePageFilter,
    TenantUsageRepository, UserFilter, UserImportArtifact, UserImportFilter, UserImportRepository,
    UserRepository, ValidatedTenantDataBackup, database_utc_now, validate_cache_namespace,
};
#[cfg(feature = "repositories")]
pub mod application_ports;
