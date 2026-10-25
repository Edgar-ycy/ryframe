mod app_config;
mod auth_config;
mod bootstrap_config;
mod configuration_args;
mod cors_config;
mod data_retention_config;
mod db_config;
mod environment;
mod exposure_config;
mod job_config;
mod logger_config;
mod messaging_config;
mod multi_tenancy_config;
mod object_storage_config;
mod pagination_config;
mod rate_limit_config;
mod redis_config;
#[cfg(feature = "tools")]
mod release_stage;
mod reset_config;
mod resource_scope;
mod runtime_config;
mod telemetry_config;
mod tenant_config_transfer_config;
mod tenant_data_config;
#[cfg(feature = "tools")]
mod tool_config;
mod user_import_config;
#[cfg(feature = "tools")]
pub use release_stage::{ReleaseStage, release_stage, stable_version, workspace_version};
#[cfg(feature = "tools")]
pub use tool_config::{GeneratorSettings, ToolSettings, VerificationSettings, WorkspaceSettings};

pub use app_config::AppSettings;
pub use auth_config::AuthConfig;
pub use bootstrap_config::BootstrapConfig;
pub use configuration_args::ConfigurationArgs;
pub use cors_config::CorsConfig;
pub use data_retention_config::DataRetentionConfig;
pub use db_config::{
    DatabaseConfig, DatabaseReplicaConfig, DatabaseSourceConfig, DbConnection, DbTlsMode,
    MigrationMode, SqlLogLevel,
};
pub use environment::Environment;
pub use exposure_config::{ApiDocsConfig, MonitorConfig};
pub use job_config::{JobConfig, JobWorkerMode, MAX_EXPORT_ROWS, MAX_XLSX_DATA_ROWS};
pub use logger_config::{LoggerConfig, LoggerFormat, LoggerLevel, LoggerOutput};
pub use messaging_config::MessagingConfig;
pub use multi_tenancy_config::{MultiTenancyConfig, SINGLE_TENANT_ID};
pub use object_storage_config::{ObjectStorageConfig, StorageBackend};
pub use pagination_config::PaginationConfig;
pub use rate_limit_config::RateLimitConfig;
pub use redis_config::{RedisConfig, RedisMode};
pub use reset_config::ResetConfig;
pub use resource_scope::ResourceScopeId;
pub use runtime_config::{ProxyConfig, UploadLimitsConfig};
pub use telemetry_config::{TelemetryConfig, TelemetrySpanLimits};
pub use tenant_config_transfer_config::TenantConfigTransferConfig;
pub use tenant_data_config::{
    SHARED_CONTROL_TARGET_KEY, TenantDataConfig, TenantDatabaseTargetConfig,
    TenantDatabaseTargetKind, TenantDatabaseTargetMode, is_valid_target_key,
};
pub use user_import_config::UserImportConfig;

pub use crate::app_config::AppConfig;

#[cfg(all(test, feature = "tools"))]
fn test_directory(prefix: &str) -> tempfile::TempDir {
    let parent =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.local-tests/config-fixtures");
    std::fs::create_dir_all(&parent).expect("创建工作区配置测试目录");
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(parent)
        .expect("创建隔离配置测试目录")
}
