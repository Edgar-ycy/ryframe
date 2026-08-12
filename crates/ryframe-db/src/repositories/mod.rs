#[macro_use]
mod macros;

pub mod background_job_repo;
pub mod cache_namespace_version_repo;
pub mod config_repo;
pub mod data_retention_repo;
pub mod dept_repo;
pub mod dict_repo;
pub mod export_job_repo;
pub mod file_repo;
pub mod job_schedule_repo;
mod login_info_repo;
pub mod menu_repo;
pub mod message_repo;
pub mod notice_repo;
mod oper_log_repo;
pub mod outbox_event_repo;
pub mod overview_repo;
pub mod password_reset_request_repo;
pub mod permission_repo;
pub mod post_repo;
pub mod role_repo;
pub mod tenant_config_transfer_repo;
pub mod tenant_provisioning_repo;
pub mod tenant_repo;
pub mod user_import_repo;
pub mod user_repo;

pub use background_job_repo::{
    BackgroundJobFilter, BackgroundJobRepository, BackgroundJobStats, BackgroundJobTypeStats,
    EnqueueBackgroundJob, EnqueueBackgroundJobResult, ExpiredLeaseRecovery, FailBackgroundJob,
    JobFailureDisposition,
};
pub use cache_namespace_version_repo::{
    CONFIG_CACHE_NAMESPACE, CacheNamespaceVersionRepository, validate_cache_namespace,
};
pub use config_repo::{ConfigFilter, ConfigRepository};
pub use data_retention_repo::{
    DataRetentionRepository, RetentionCleanupResult, RetentionCutoff, RetentionResource,
};
pub use dept_repo::DeptRepository;
pub use dict_repo::{DictDataRepository, DictTypeFilter, DictTypeRepository};
pub use export_job_repo::{CreateExportJob, ExportJobRepository, MarkExportJobSucceeded};
pub use file_repo::FileRepository;
pub use job_schedule_repo::{JobScheduleExecutionFilter, JobScheduleFilter, JobScheduleRepository};
pub use login_info_repo::{LoginInfoFilter, LoginInfoRepository};
pub use menu_repo::{MenuFilter, MenuRepository};
pub use message_repo::{
    MessageAudienceKind, MessageAudienceSelector, MessageInboxQuery, MessageRepository,
    PublishMessageCommand, PublishedMessage, RecipientMessage, RecipientMessagePage,
};
pub use notice_repo::{NoticeFilter, NoticeRepository};
pub use oper_log_repo::{OperLogFilter, OperLogRepository};
pub use outbox_event_repo::{OutboxEventRepository, OutboxFailureDisposition, RecordOutboxEvent};
pub use overview_repo::{OverviewRepository, OverviewTrendCount, ScheduleOverviewStats};
pub use password_reset_request_repo::PasswordResetRequestRepository;
pub use permission_repo::PermissionRepository;
pub use post_repo::{PostFilter, PostRepository};
pub use role_repo::{RoleFilter, RoleRepository};
pub use tenant_config_transfer_repo::{TenantConfigTransferRepository, TenantConfigurationFence};
pub use tenant_provisioning_repo::{ProvisionTenantCommand, TenantProvisioningRepository};
pub use tenant_repo::TenantRepository;
pub use user_import_repo::{
    CreateUserImportJob, UserImportArtifact, UserImportFilter, UserImportRepository,
};
pub use user_repo::{UserFilter, UserRepository};

/// 构造把 `%`、`_` 和转义符视为普通字符的 SQL 前缀匹配表达式。
pub(crate) fn prefix_like(value: &str) -> sea_orm::sea_query::LikeExpr {
    let escaped = value
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_");
    sea_orm::sea_query::LikeExpr::new(format!("{escaped}%")).escape('!')
}
