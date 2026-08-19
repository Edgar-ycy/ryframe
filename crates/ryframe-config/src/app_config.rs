use serde::Deserialize;

use crate::{
    ApiDocsConfig, AuthConfig, CorsConfig, DataRetentionConfig, DatabaseConfig, Environment,
    JobConfig, LoggerConfig, MessagingConfig, MonitorConfig, MultiTenancyConfig,
    ObjectStorageConfig, PaginationConfig, ProxyConfig, RateLimitConfig, RedisConfig,
    ServiceAccountsConfig, TelemetryConfig, TenantConfigTransferConfig, TenantDataConfig,
    UploadLimitsConfig, UserImportConfig,
};

mod defaults;
mod environment_overrides;
mod loader;
mod security;
mod validation;

/// 应用基础配置
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSettings {
    /// 应用名称
    pub name: String,
    /// 监听地址
    pub host: String,
    /// 监听端口
    pub port: u16,
}

/// 顶层应用配置
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    /// 当前进程唯一确定的运行环境，不参与配置文件反序列化。
    #[serde(skip)]
    pub environment: Environment,
    /// 当前进程使用的 Snowflake worker ID，不参与配置文件反序列化。
    #[serde(skip)]
    pub snowflake_worker_id: i64,
    pub app: AppSettings,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    #[serde(default)]
    pub multi_tenancy: MultiTenancyConfig,
    #[serde(default)]
    pub redis: Option<RedisConfig>,
    pub logger: LoggerConfig,
    #[serde(default)]
    pub rate_limit: RateLimitConfig,
    #[serde(default)]
    pub pagination: PaginationConfig,
    #[serde(default)]
    pub cors: CorsConfig,
    #[serde(default)]
    pub object_storage: ObjectStorageConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub upload: UploadLimitsConfig,
    #[serde(default)]
    pub api_docs: ApiDocsConfig,
    #[serde(default)]
    pub monitor: MonitorConfig,
    #[serde(default)]
    pub jobs: JobConfig,
    #[serde(default)]
    pub data_retention: DataRetentionConfig,
    #[serde(default)]
    pub user_import: UserImportConfig,
    #[serde(default)]
    pub tenant_config_transfer: TenantConfigTransferConfig,
    #[serde(default)]
    pub tenant_data: TenantDataConfig,
    #[serde(default)]
    pub service_accounts: ServiceAccountsConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub messaging: MessagingConfig,
}
