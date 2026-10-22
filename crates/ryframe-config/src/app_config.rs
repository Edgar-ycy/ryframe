use std::path::PathBuf;

use serde::Deserialize;

use crate::{
    ApiDocsConfig, AuthConfig, BootstrapConfig, CorsConfig, DataRetentionConfig, DatabaseConfig,
    Environment, JobConfig, LoggerConfig, MessagingConfig, MonitorConfig, MultiTenancyConfig,
    ObjectStorageConfig, PaginationConfig, ProxyConfig, RateLimitConfig, RedisConfig, ResetConfig,
    ResourceScopeId, TelemetryConfig, TenantConfigTransferConfig, TenantDataConfig,
    UploadLimitsConfig, UserImportConfig,
};

mod defaults;
mod diagnostics;
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
    /// 本地化资源目录；相对路径以当前进程工作目录解析。
    #[serde(default = "defaults::default_locales_dir")]
    pub locales_dir: PathBuf,
}

/// 顶层应用配置
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    /// 当前进程唯一确定的运行环境，不参与配置文件反序列化。
    #[serde(skip)]
    pub environment: Environment,
    /// 当前进程使用的 Snowflake worker ID，每个运行实例须配置不同值。
    pub snowflake_worker_id: i64,
    /// 仅显式初始化调用方使用；正常启动不要求保留管理员初始密码。
    #[serde(default)]
    pub bootstrap: BootstrapConfig,
    /// 共享 MySQL、Redis 和对象存储资源的强制环境作用域。
    pub scope_id: ResourceScopeId,
    /// 仅供 feature-gated 非生产重建命令读取的安全声明。
    #[serde(default)]
    pub reset: ResetConfig,
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
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub messaging: MessagingConfig,
}

impl std::fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("environment", &self.environment)
            .field("snowflake_worker_id", &self.snowflake_worker_id)
            .field("scope_id", &self.scope_id)
            .finish_non_exhaustive()
    }
}
