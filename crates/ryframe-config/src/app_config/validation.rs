use std::collections::HashSet;

use ryframe_kernel::{AppError, AppResult};

use super::security::MIN_PRODUCTION_JWT_SECRET_BYTES;
use crate::{
    AppConfig, DbConnection, DbTlsMode, MigrationMode, RedisConfig, RedisMode, StorageBackend,
    TenantDatabaseTargetKind,
};

impl AppConfig {
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
        self.database.validate().map_err(AppError::Config)?;
        self.tenant_data.validate().map_err(AppError::Config)?;
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
        if self.environment.is_production() && self.database.migration_mode != MigrationMode::Verify
        {
            return Err(AppError::Config(
                "production requires database.migration_mode = \"verify\"; schema changes must be applied by ryframe-migrate"
                    .into(),
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
        let mut tenant_databases = HashSet::with_capacity(
            self.tenant_data.targets.len()
                + self.database.replicas.len()
                + self.database.sources.len()
                + 1,
        );
        tenant_databases.insert((
            self.database.primary.host.trim(),
            self.database.primary.port,
            self.database.primary.database.trim(),
        ));
        for replica in &self.database.replicas {
            tenant_databases.insert((
                replica.connection.host.trim(),
                replica.connection.port,
                replica.connection.database.trim(),
            ));
        }
        for source in &self.database.sources {
            tenant_databases.insert((
                source.connection.host.trim(),
                source.connection.port,
                source.connection.database.trim(),
            ));
        }
        for (index, target) in self.tenant_data.targets.iter().enumerate() {
            if target.kind != TenantDatabaseTargetKind::Mysql {
                continue;
            }
            let host = target.host.as_deref().unwrap_or_default().trim();
            // 与实际连接构建保持同一规范身份；省略 MySQL 端口等价于显式 3306。
            let port = target.port.unwrap_or(3306);
            let database = target.database.as_deref().unwrap_or_default().trim();
            let identity = (host, port, database);
            if !tenant_databases.insert(identity) {
                return Err(AppError::Config(format!(
                    "tenant_data.targets[{index}] 与控制库拓扑、命名数据源或其他租户目标指向同一个 MySQL schema"
                )));
            }
            if self.environment.is_production()
                && !is_loopback_host(host)
                && target.tls_mode.unwrap_or_default() != DbTlsMode::VerifyIdentity
            {
                return Err(AppError::Config(format!(
                    "remote production tenant_data.targets[{index}] requires tls_mode = \"verify_identity\""
                )));
            }
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
        self.data_retention.validate().map_err(AppError::Config)?;
        self.user_import
            .validate(self.upload.file_max_bytes)
            .map_err(AppError::Config)?;
        self.tenant_config_transfer
            .validate(self.upload.file_max_bytes)
            .map_err(AppError::Config)?;
        self.service_accounts.validate().map_err(AppError::Config)?;
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
        if self.service_accounts.enabled
            && !self
                .redis
                .as_ref()
                .is_some_and(|redis| redis.mode == RedisMode::Required)
        {
            return Err(AppError::Config(
                "启用 service_accounts 时要求 redis.mode = \"required\"".into(),
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
        if self.environment.is_production() && self.service_accounts.enabled {
            self.service_accounts
                .load_pepper_keyring(jwt_secret)
                .map_err(AppError::Config)?;
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
            StorageBackend::Local => {
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
            StorageBackend::Rustfs | StorageBackend::Minio | StorageBackend::S3 => {
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

pub(super) fn reject_removed_database_fields(table: &toml::Table) -> AppResult<()> {
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
    connection: &DbConnection,
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
        DbTlsMode::VerifyCa | DbTlsMode::VerifyIdentity
    ) && non_empty(connection.tls_ca.as_deref()).is_none()
    {
        return Err(AppError::Config(format!(
            "{path}.tls_ca is required for certificate verification"
        )));
    }
    if production
        && !is_loopback_host(&connection.host)
        && connection.tls_mode != DbTlsMode::VerifyIdentity
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
