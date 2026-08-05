use std::{collections::HashSet, path::Path};

use ryframe_kernel::{AppError, AppResult};
use serde::Deserialize;

use crate::{
    ApiDocsConfig, AuthConfig, CorsConfig, DatabaseConfig, Environment, GeneratorConfig, JobConfig,
    LoggerConfig, MessagingConfig, MigrationMode, MonitorConfig, ObjectStorageConfig,
    PaginationConfig, ProxyConfig, RateLimitConfig, RedisConfig, RedisMode, TelemetryConfig,
    UploadLimitsConfig,
};

mod environment_overrides;

use environment_overrides::apply_env_overrides;

const MIN_PRODUCTION_JWT_SECRET_BYTES: usize = 32;

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

// #[derive(Default)] 不能用于 AppSettings，需要提供有意义的应用默认值
// （名称、监听地址等），而非空字符串。
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            name: "ryframe".into(),
            host: "0.0.0.0".into(),
            port: 8080,
        }
    }
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
    #[serde(default)]
    pub generator: GeneratorConfig,
    pub auth: AuthConfig,
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
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub messaging: MessagingConfig,
}

impl AppConfig {
    /// 加载配置：app.toml → app.{env}.toml → APP_* 环境变量
    ///
    /// `config_dir` 为配置文件所在目录的路径（如 `"config"` 或 `"/app/config"`）。
    /// 环境配置文件仅需包含要覆盖的字段，不要求完整。
    pub fn load(config_dir: impl AsRef<Path>, environment: Environment) -> AppResult<Self> {
        let mut table = load_merged_table(config_dir.as_ref(), environment)?;
        if environment.is_production() {
            reject_production_file_secrets(&table)?;
        }
        apply_env_overrides(&mut table)?;
        reject_removed_secret_encoding(&table)?;
        let migration_mode_was_explicit = table
            .get("database")
            .and_then(toml::Value::as_table)
            .is_some_and(|database| database.contains_key("migration_mode"));
        let job_mode_was_explicit = table
            .get("jobs")
            .and_then(toml::Value::as_table)
            .is_some_and(|jobs| jobs.contains_key("mode"));
        apply_migration_mode_default(&mut table, environment);
        apply_job_mode_default(&mut table, environment);
        reject_removed_database_fields(&table)?;

        let mut config: AppConfig = table
            .try_into()
            .map_err(|e| AppError::Config(format!("配置反序列化失败: {}", e)))?;

        config.environment = environment;
        config.snowflake_worker_id = resolve_snowflake_worker_id(environment)?;

        // 生产敏感值只允许由环境变量或 `_FILE` 指向的外部 secret 注入，加载后直接校验最终值。
        config.validate()?;
        if environment.is_production()
            && migration_mode_was_explicit
            && config.database.migration_mode != MigrationMode::Verify
        {
            return Err(AppError::Config(
                "production requires database.migration_mode = \"verify\"; run ryframe-migrate up before starting the API".into(),
            ));
        }
        if environment.is_production()
            && job_mode_was_explicit
            && config.jobs.mode != crate::JobWorkerMode::External
        {
            return Err(AppError::Config(
                "生产环境 jobs.mode 必须为 \"external\"；请使用独立的 ryframe-worker 进程消费任务"
                    .into(),
            ));
        }

        Ok(config)
    }

    /// 从 `APP_CONFIG_DIR` 加载配置，未设置时默认使用 `config`。
    ///
    /// 相对路径仍以进程工作目录为基准，既保留 `load("config")` 的既有行为，
    /// 也允许容器显式挂载配置目录。
    pub fn load_from_env(environment: Environment) -> AppResult<Self> {
        match std::env::var("APP_CONFIG_DIR") {
            Ok(config_dir) if config_dir.trim().is_empty() => Err(AppError::Config(
                "APP_CONFIG_DIR must not be empty when it is set".into(),
            )),
            Ok(config_dir) => Self::load(config_dir, environment),
            Err(std::env::VarError::NotPresent) => Self::load("config", environment),
            Err(std::env::VarError::NotUnicode(_)) => Err(AppError::Config(
                "APP_CONFIG_DIR must contain valid Unicode".into(),
            )),
        }
    }

    /// 校验必填配置项
    pub fn validate(&self) -> AppResult<()> {
        ryframe_utils::snowflake::validate_worker_id(self.snowflake_worker_id)
            .map_err(|error| AppError::Config(error.to_string()))?;
        if self.app.name.is_empty() {
            return Err(AppError::Config("app.name 不能为空".into()));
        }
        if self.app.host.is_empty() {
            return Err(AppError::Config("app.host 不能为空".into()));
        }
        if self.app.port == 0 {
            return Err(AppError::Config("app.port 必须大于 0".into()));
        }
        validate_database_connection(
            "database.primary",
            &self.database.primary,
            self.environment.is_production(),
        )?;
        if !self.environment.is_test() && self.database.migration_mode == MigrationMode::Off {
            return Err(AppError::Config(
                "database.migration_mode = \"off\" is allowed only when APP_ENV=test".into(),
            ));
        }

        let mut replica_names = HashSet::with_capacity(self.database.replicas.len());
        for (index, replica) in self.database.replicas.iter().enumerate() {
            let name = replica.name.trim();
            if name.is_empty() {
                return Err(AppError::Config(format!(
                    "database.replicas[{index}].name 不能为空"
                )));
            }
            if !replica_names.insert(name) {
                return Err(AppError::Config(format!(
                    "database.replicas 名称重复: {name}"
                )));
            }
            validate_database_connection(
                &format!("database.replicas[{index}]"),
                &replica.connection,
                self.environment.is_production(),
            )?;
        }
        let mut source_names = HashSet::with_capacity(self.database.sources.len());
        for (index, source) in self.database.sources.iter().enumerate() {
            let name = source.name.trim();
            if name.is_empty() {
                return Err(AppError::Config(format!(
                    "database.sources[{index}].name 不能为空"
                )));
            }
            if name == "primary" {
                return Err(AppError::Config(
                    "database.sources 名称不能使用保留名称 primary".into(),
                ));
            }
            if !source_names.insert(name) {
                return Err(AppError::Config(format!(
                    "database.sources 名称重复: {name}"
                )));
            }
            if replica_names.contains(name) {
                return Err(AppError::Config(format!(
                    "database.sources 与 database.replicas 名称冲突: {name}"
                )));
            }
            validate_database_connection(
                &format!("database.sources[{index}]"),
                &source.connection,
                self.environment.is_production(),
            )?;
        }
        let generator_source = self.generator.data_source.trim();
        if generator_source.is_empty() {
            return Err(AppError::Config("generator.data_source 不能为空".into()));
        }
        if generator_source != "primary" && !source_names.contains(generator_source) {
            return Err(AppError::Config(format!(
                "generator.data_source 未注册: {generator_source}"
            )));
        }
        let jwt_secret = self.auth.jwt_secret.trim();
        if jwt_secret.is_empty() {
            return Err(AppError::Config("auth.jwt_secret 不能为空".into()));
        }
        if self.environment.is_production() {
            if jwt_secret == "change-me-in-production" {
                return Err(AppError::Config(
                    "生产环境必须修改 auth.jwt_secret，不允许使用默认值".into(),
                ));
            }
            if jwt_secret.len() < MIN_PRODUCTION_JWT_SECRET_BYTES {
                return Err(AppError::Config(format!(
                    "生产环境 auth.jwt_secret 至少需要 {MIN_PRODUCTION_JWT_SECRET_BYTES} 字节"
                )));
            }
        }
        if self.auth.max_login_attempts == 0 || self.auth.lockout_duration_minutes == 0 {
            return Err(AppError::Config(
                "auth.max_login_attempts 和 auth.lockout_duration_minutes 必须大于 0".into(),
            ));
        }
        self.rate_limit.validate().map_err(AppError::Config)?;
        self.pagination.validate().map_err(AppError::Config)?;
        self.logger.validate().map_err(AppError::Config)?;
        self.jobs
            .validate(self.environment)
            .map_err(AppError::Config)?;
        self.telemetry.validate().map_err(AppError::Config)?;
        self.messaging.validate().map_err(AppError::Config)?;
        let access_ttl =
            parse_duration_seconds("auth.access_token_expire", &self.auth.access_token_expire)?;
        let refresh_ttl =
            parse_duration_seconds("auth.refresh_token_expire", &self.auth.refresh_token_expire)?;
        if access_ttl == 0 || refresh_ttl == 0 {
            return Err(AppError::Config(
                "auth token expiry durations must be greater than zero".into(),
            ));
        }
        if refresh_ttl > 7 * 24 * 60 * 60 {
            return Err(AppError::Config(
                "auth.refresh_token_expire cannot exceed the 7-day absolute session limit".into(),
            ));
        }
        if self.environment.is_production()
            && self.messaging.enabled
            && !self
                .redis
                .as_ref()
                .is_some_and(|redis| redis.mode == RedisMode::Required)
        {
            return Err(AppError::Config(
                "production messaging requires redis.mode = \"required\"".into(),
            ));
        }
        if self.environment.is_production()
            && !self
                .redis
                .as_ref()
                .is_some_and(|redis| redis.mode == RedisMode::Required)
        {
            return Err(AppError::Config(
                "production requires redis.mode = \"required\"".into(),
            ));
        }
        if let Some(redis) = &self.redis {
            validate_redis_tls(redis, self.environment.is_production())?;
        }
        if self.environment.is_production() && self.cors.allow_origins.is_empty() {
            return Err(AppError::Config(
                "production requires at least one explicit CORS origin".into(),
            ));
        }
        if self.environment.is_production() && self.api_docs.enabled {
            return Err(AppError::Config(
                "production requires api_docs.enabled = false".into(),
            ));
        }
        if self.environment.is_production() && self.monitor.metrics_bearer_token.trim().len() < 32 {
            return Err(AppError::Config(
                "production monitor.metrics_bearer_token must be at least 32 bytes".into(),
            ));
        }
        for origin in &self.cors.allow_origins {
            validate_origin(origin, self.environment.is_production())?;
        }
        ryframe_utils::ip::TrustedProxySet::new(&self.proxy.trusted_cidrs)
            .map_err(AppError::Config)?;
        if self.upload.avatar_max_bytes == 0
            || self.upload.file_max_bytes == 0
            || self.upload.avatar_max_bytes > self.upload.file_max_bytes
            || self.upload.multipart_envelope_bytes == 0
            || self.upload.api_timeout_seconds == 0
            || self.upload.upload_timeout_seconds < self.upload.api_timeout_seconds
        {
            return Err(AppError::Config(
                "invalid upload limits or timeout configuration".into(),
            ));
        }
        match self.object_storage.backend {
            crate::StorageBackend::Local => {
                if self.object_storage.local_base_dir.trim().is_empty() {
                    return Err(AppError::Config(
                        "object_storage.local_base_dir 不能为空".into(),
                    ));
                }
                if self.environment.is_production()
                    && !self.object_storage.allow_local_in_production
                {
                    return Err(AppError::Config(
                        "production local object storage requires \
                         object_storage.allow_local_in_production = true"
                            .into(),
                    ));
                }
            }
            crate::StorageBackend::Rustfs
            | crate::StorageBackend::Minio
            | crate::StorageBackend::S3 => {
                if self.object_storage.endpoint.trim().is_empty()
                    || self.object_storage.access_key.trim().is_empty()
                    || self.object_storage.secret_key.is_empty()
                    || self.object_storage.region.trim().is_empty()
                {
                    return Err(AppError::Config(
                        "RustFS/MinIO/S3 需要 endpoint、access_key、secret_key 和 region".into(),
                    ));
                }
                if self.environment.is_production() && !self.object_storage.use_ssl {
                    return Err(AppError::Config(
                        "生产环境的 RustFS/MinIO/S3 必须设置 object_storage.use_ssl = true".into(),
                    ));
                }
                if self.environment.is_production()
                    && self
                        .object_storage
                        .endpoint
                        .trim()
                        .to_ascii_lowercase()
                        .starts_with("http://")
                {
                    return Err(AppError::Config(
                        "生产环境的 RustFS/MinIO/S3 端点必须使用 HTTPS".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

const PRODUCTION_FILE_SECRET_KEYS: &[&str] = &[
    "password",
    "jwt_secret",
    "access_key",
    "secret_key",
    "metrics_bearer_token",
];

fn reject_production_file_secrets(table: &toml::Table) -> AppResult<()> {
    inspect_production_file_secrets(&toml::Value::Table(table.clone()), "")
}

fn inspect_production_file_secrets(value: &toml::Value, path: &str) -> AppResult<()> {
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                if PRODUCTION_FILE_SECRET_KEYS.contains(&key.as_str())
                    && let toml::Value::String(secret) = child
                    && !secret.is_empty()
                    && !(key == "jwt_secret" && secret == "change-me-in-production")
                {
                    return Err(AppError::Config(format!(
                        "生产配置文件不得包含敏感值 {child_path}；请使用对应 APP_* 环境变量或外部 secret manager 注入"
                    )));
                }
                inspect_production_file_secrets(child, &child_path)?;
            }
        }
        toml::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                inspect_production_file_secrets(child, &format!("{path}[{index}]"))?;
            }
        }
        _ => {}
    }

    Ok(())
}

fn reject_removed_secret_encoding(table: &toml::Table) -> AppResult<()> {
    inspect_removed_secret_encoding(&toml::Value::Table(table.clone()), "")
}

fn inspect_removed_secret_encoding(value: &toml::Value, path: &str) -> AppResult<()> {
    match value {
        toml::Value::String(value) if value.starts_with("ENC[") => Err(AppError::Config(format!(
            "配置 {path} 使用了已删除的 ENC[...] 格式；请通过 APP_* 环境变量或外部 secret manager 注入原始值"
        ))),
        toml::Value::Table(table) => {
            for (key, child) in table {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                inspect_removed_secret_encoding(child, &child_path)?;
            }
            Ok(())
        }
        toml::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                inspect_removed_secret_encoding(child, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn parse_duration_seconds(path: &str, raw: &str) -> AppResult<u64> {
    let value = raw.trim();
    let (number, multiplier) = if let Some(hours) = value.strip_suffix('h') {
        (hours.trim(), 60_u64 * 60)
    } else if let Some(minutes) = value.strip_suffix('m') {
        (minutes.trim(), 60)
    } else if let Some(seconds) = value.strip_suffix('s') {
        (seconds.trim(), 1)
    } else {
        (value, 1)
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|duration| duration.checked_mul(multiplier))
        .ok_or_else(|| AppError::Config(format!("{path} is not a valid duration: {raw}")))
}

fn reject_removed_database_fields(table: &toml::Table) -> AppResult<()> {
    let Some(database) = table.get("database") else {
        return Ok(());
    };
    if contains_key(database, "driver") {
        return Err(AppError::Config(
            "database.driver was removed in v0.5; RyFrame supports MySQL only".into(),
        ));
    }
    Ok(())
}

fn resolve_snowflake_worker_id(environment: Environment) -> AppResult<i64> {
    match std::env::var("SNOWFLAKE_WORKER_ID") {
        Ok(value) => {
            let worker_id = value.trim().parse::<i64>().map_err(|_| {
                AppError::Config(format!(
                    "SNOWFLAKE_WORKER_ID 必须是 0~{} 的整数，当前值: {value}",
                    ryframe_utils::snowflake::MAX_WORKER_ID
                ))
            })?;
            ryframe_utils::snowflake::validate_worker_id(worker_id)
                .map_err(|error| AppError::Config(error.to_string()))?;
            Ok(worker_id)
        }
        Err(std::env::VarError::NotPresent) if environment.is_production() => {
            Err(AppError::Config(
                "生产环境必须显式设置 SNOWFLAKE_WORKER_ID，且每个应用实例必须使用不同值".into(),
            ))
        }
        Err(std::env::VarError::NotPresent) => Ok(1),
        Err(std::env::VarError::NotUnicode(_)) => Err(AppError::Config(
            "SNOWFLAKE_WORKER_ID 必须是有效的 UTF-8 整数".into(),
        )),
    }
}

fn apply_migration_mode_default(table: &mut toml::Table, environment: Environment) {
    let Some(toml::Value::Table(database)) = table.get_mut("database") else {
        return;
    };
    database.entry("migration_mode").or_insert_with(|| {
        toml::Value::String(
            if environment.is_production() {
                "verify"
            } else {
                "auto"
            }
            .into(),
        )
    });
}

fn apply_job_mode_default(table: &mut toml::Table, environment: Environment) {
    let jobs = table
        .entry("jobs")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    let toml::Value::Table(jobs) = jobs else {
        return;
    };
    jobs.entry("mode").or_insert_with(|| {
        toml::Value::String(
            if environment.is_production() {
                "external"
            } else {
                "embedded"
            }
            .into(),
        )
    });
}

fn contains_key(value: &toml::Value, rejected: &str) -> bool {
    match value {
        toml::Value::Table(table) => {
            table.contains_key(rejected)
                || table.values().any(|value| contains_key(value, rejected))
        }
        toml::Value::Array(values) => values.iter().any(|value| contains_key(value, rejected)),
        _ => false,
    }
}

fn validate_origin(origin: &str, production: bool) -> AppResult<()> {
    let (scheme, authority) = origin
        .split_once("://")
        .ok_or_else(|| AppError::Config(format!("invalid CORS origin: {origin}")))?;
    if !matches!(scheme, "http" | "https")
        || authority.is_empty()
        || authority.contains('/')
        || authority.contains('*')
        || authority.chars().any(char::is_whitespace)
        || (production && scheme != "https")
    {
        return Err(AppError::Config(format!(
            "CORS origin must be a complete{} origin without path or wildcard: {origin}",
            if production { " HTTPS" } else { "" }
        )));
    }
    Ok(())
}

fn validate_database_connection(
    path: &str,
    connection: &crate::DbConnection,
    production: bool,
) -> AppResult<()> {
    if connection.database.trim().is_empty() {
        return Err(AppError::Config(format!("{path}.database 不能为空")));
    }
    if connection.host.trim().is_empty() {
        return Err(AppError::Config(format!("{path}.host 不能为空")));
    }
    if connection.port == 0 {
        return Err(AppError::Config(format!("{path}.port 必须大于 0")));
    }
    if connection.username.trim().is_empty() {
        return Err(AppError::Config(format!("{path}.username 不能为空")));
    }
    if connection.max_connections == 0 {
        return Err(AppError::Config(format!(
            "{path}.max_connections 必须大于 0"
        )));
    }
    if connection.min_connections > connection.max_connections {
        return Err(AppError::Config(format!(
            "{path}.min_connections 不能大于 max_connections"
        )));
    }
    if connection.acquire_timeout_secs == 0 || connection.connect_timeout_secs == 0 {
        return Err(AppError::Config(format!(
            "{path} 的 acquire_timeout_secs 和 connect_timeout_secs 必须大于 0"
        )));
    }
    let client_cert = non_empty(connection.tls_client_cert.as_deref());
    let client_key = non_empty(connection.tls_client_key.as_deref());
    if client_cert.is_some() != client_key.is_some() {
        return Err(AppError::Config(format!(
            "{path}.tls_client_cert and tls_client_key must be configured together"
        )));
    }
    if matches!(
        connection.tls_mode,
        crate::DbTlsMode::VerifyCa | crate::DbTlsMode::VerifyIdentity
    ) && non_empty(connection.tls_ca.as_deref()).is_none()
    {
        return Err(AppError::Config(format!(
            "{path}.tls_ca is required for certificate verification"
        )));
    }
    if production
        && !is_loopback_host(&connection.host)
        && connection.tls_mode != crate::DbTlsMode::VerifyIdentity
    {
        return Err(AppError::Config(format!(
            "remote production {path} requires tls_mode = \"verify_identity\""
        )));
    }
    Ok(())
}

fn validate_redis_tls(redis: &RedisConfig, production: bool) -> AppResult<()> {
    let client_cert = non_empty(redis.tls_client_cert.as_deref());
    let client_key = non_empty(redis.tls_client_key.as_deref());
    if client_cert.is_some() != client_key.is_some() {
        return Err(AppError::Config(
            "redis.tls_client_cert and tls_client_key must be configured together".into(),
        ));
    }
    if !redis.tls
        && (non_empty(redis.tls_ca.as_deref()).is_some()
            || client_cert.is_some()
            || client_key.is_some())
    {
        return Err(AppError::Config(
            "Redis TLS certificate paths require redis.tls = true".into(),
        ));
    }
    if production && !is_loopback_host(&redis.host) && !redis.tls {
        return Err(AppError::Config(
            "remote production Redis requires redis.tls = true".into(),
        ));
    }
    Ok(())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn load_merged_table(config_dir: &Path, environment: Environment) -> AppResult<toml::Table> {
    // 第一层：将默认配置加载为 TOML 表。
    let base_path = config_dir.join("app.toml");
    let base_toml = std::fs::read_to_string(&base_path)
        .map_err(|e| AppError::Config(format!("无法读取 {}: {}", base_path.display(), e)))?;
    let mut table: toml::Table = toml::from_str(&base_toml)
        .map_err(|e| AppError::Config(format!("解析 {} 失败: {}", base_path.display(), e)))?;

    // 第二层：加载环境配置文件并合并到基础表。
    let env_path = config_dir.join(format!("app.{}.toml", environment.as_str()));
    match std::fs::read_to_string(&env_path) {
        Ok(env_toml) => {
            let env_table: toml::Table = toml::from_str(&env_toml).map_err(|e| {
                AppError::Config(format!("解析 {} 失败: {}", env_path.display(), e))
            })?;
            merge_tables(&mut table, &env_table);
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound && !environment.is_production() => {}
        Err(error) => {
            return Err(AppError::Config(format!(
                "无法读取环境配置 {}: {}",
                env_path.display(),
                error
            )));
        }
    }

    Ok(table)
}

/// 递归合并两个 TOML 表，环境配置的值覆盖基础配置中对应位置的值。
///
/// - 表 → 递归合并子键。
/// - 其他值 → 环境配置直接覆盖基础配置。
fn merge_tables(base: &mut toml::Table, env: &toml::Table) {
    for (key, value) in env {
        match (base.get_mut(key), value) {
            // 两端均为表时递归合并。
            (Some(toml::Value::Table(base_table)), toml::Value::Table(env_table)) => {
                merge_tables(base_table, env_table);
            }
            // 环境配置覆盖基础配置。
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}
