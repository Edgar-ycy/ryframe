use std::collections::HashSet;

use ryframe_common::{AppError, AppResult};
use serde::Deserialize;

use crate::{
    AuthConfig, CorsConfig, DatabaseConfig, GeneratorConfig, LoggerConfig, ObjectStorageConfig,
    RateLimitConfig, RedisConfig,
};

/// 应用基础配置
#[derive(Debug, Clone, Deserialize)]
pub struct AppSettings {
    /// 应用名称
    pub name: String,
    /// 版本号
    pub version: String,
    /// 监听地址
    pub host: String,
    /// 监听端口
    pub port: u16,
}

// #[derive(Default)] 不能用于 AppSettings，需要提供有意义的应用默认值
// （名称、版本号等），而非空字符串。
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            name: "ryframe".into(),
            version: "0.1.0".into(),
            host: "0.0.0.0".into(),
            port: 8080,
        }
    }
}

/// 顶层应用配置
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
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
    pub cors: CorsConfig,
    #[serde(default)]
    pub object_storage: ObjectStorageConfig,
}

impl AppConfig {
    /// 加载配置：app.toml → app.{env}.toml → APP_* 环境变量
    ///
    /// `config_dir` 为配置文件所在目录的路径（如 `"config"` 或 `"/app/config"`）。
    /// 环境配置文件仅需包含要覆盖的字段，不要求完整。
    pub fn load(config_dir: &str) -> AppResult<Self> {
        let env = std::env::var("APP_ENV").unwrap_or_else(|_| "dev".to_string());
        let mut table = load_merged_table(config_dir, &env)?;
        apply_env_overrides(&mut table)?;

        let mut config: AppConfig = table
            .try_into()
            .map_err(|e| AppError::Config(format!("配置反序列化失败: {}", e)))?;

        // 校验
        config.validate(&env)?;

        // 解密敏感配置字段（如果 CONFIG_MASTER_KEY 已设置）
        crate::config_crypto::decrypt_config(&mut config)?;

        Ok(config)
    }

    /// 仅重新加载可热更新的配置字段（不包含 database、app.host/port 等）
    ///
    /// 复用完整加载流程，确保环境配置文件可以只写差异字段，并继续支持 APP_* 覆盖。
    /// 调用方仍只应用可热更新字段。
    pub fn reload_hot(config_dir: &str) -> AppResult<Self> {
        Self::load(config_dir)
    }

    /// 校验必填配置项
    pub fn validate(&self, env: &str) -> AppResult<()> {
        if self.app.name.is_empty() {
            return Err(AppError::Config("app.name 不能为空".into()));
        }
        if self.app.host.is_empty() {
            return Err(AppError::Config("app.host 不能为空".into()));
        }
        if self.app.port == 0 {
            return Err(AppError::Config("app.port 必须大于 0".into()));
        }
        validate_database_connection("database.primary", &self.database.primary)?;

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
            if replica.connection.driver != self.database.primary.driver {
                return Err(AppError::Config(format!(
                    "database.replicas[{index}].driver 必须与 database.primary.driver 一致"
                )));
            }
            validate_database_connection(
                &format!("database.replicas[{index}]"),
                &replica.connection,
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
        if self.auth.jwt_secret.is_empty() {
            return Err(AppError::Config("auth.jwt_secret 不能为空".into()));
        }
        if env == "prod" && self.auth.jwt_secret == "change-me-in-production" {
            return Err(AppError::Config(
                "生产环境必须修改 auth.jwt_secret，不允许使用默认值".into(),
            ));
        }
        match self.object_storage.backend {
            crate::StorageBackend::Local => {
                if self.object_storage.local_base_dir.trim().is_empty() {
                    return Err(AppError::Config(
                        "object_storage.local_base_dir 不能为空".into(),
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
            }
        }
        Ok(())
    }
}

fn validate_database_connection(path: &str, connection: &crate::DbConnection) -> AppResult<()> {
    if !matches!(connection.driver.as_str(), "mysql" | "postgres" | "sqlite") {
        return Err(AppError::Config(format!(
            "{path}.driver 必须是 mysql、postgres 或 sqlite"
        )));
    }
    if connection.database.trim().is_empty() {
        return Err(AppError::Config(format!("{path}.database 不能为空")));
    }
    if connection.driver != "sqlite" {
        if connection.host.trim().is_empty() {
            return Err(AppError::Config(format!("{path}.host 不能为空")));
        }
        if connection.port == 0 {
            return Err(AppError::Config(format!("{path}.port 必须大于 0")));
        }
        if connection.username.trim().is_empty() {
            return Err(AppError::Config(format!("{path}.username 不能为空")));
        }
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
    Ok(())
}

fn load_merged_table(config_dir: &str, env: &str) -> AppResult<toml::Table> {
    // 第一层：加载默认配置为 TOML Table
    let base_path = format!("{}/app.toml", config_dir);
    let base_toml = std::fs::read_to_string(&base_path)
        .map_err(|e| AppError::Config(format!("无法读取 {}: {}", base_path, e)))?;
    let mut table: toml::Table = toml::from_str(&base_toml)
        .map_err(|e| AppError::Config(format!("解析 {} 失败: {}", base_path, e)))?;

    // 第二层：加载环境配置文件，merge 到 base table
    let env_path = format!("{}/app.{}.toml", config_dir, env);
    if let Ok(env_toml) = std::fs::read_to_string(&env_path) {
        let env_table: toml::Table = toml::from_str(&env_toml)
            .map_err(|e| AppError::Config(format!("解析 {} 失败: {}", env_path, e)))?;
        merge_tables(&mut table, &env_table);
    }

    Ok(table)
}

fn apply_env_overrides(table: &mut toml::Table) -> AppResult<()> {
    for spec in ENV_OVERRIDES {
        let Ok(value) = std::env::var(spec.name) else {
            continue;
        };
        let value = parse_env_value(spec.name, &value, spec.value_type)?;
        insert_toml_path(table, spec.path, value);
    }

    Ok(())
}

#[derive(Clone, Copy)]
struct EnvOverride {
    name: &'static str,
    path: &'static [&'static str],
    value_type: EnvValueType,
}

#[derive(Clone, Copy)]
enum EnvValueType {
    String,
    Integer,
    Bool,
    StringArray,
    Json,
}

const ENV_OVERRIDES: &[EnvOverride] = &[
    EnvOverride {
        name: "APP_APP_NAME",
        path: &["app", "name"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_APP_VERSION",
        path: &["app", "version"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_APP_HOST",
        path: &["app", "host"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_APP_PORT",
        path: &["app", "port"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_SQL_LOG_LEVEL",
        path: &["database", "sql_log_level"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_DRIVER",
        path: &["database", "primary", "driver"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_HOST",
        path: &["database", "primary", "host"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_PORT",
        path: &["database", "primary", "port"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_NAME",
        path: &["database", "primary", "database"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_USERNAME",
        path: &["database", "primary", "username"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_PASSWORD",
        path: &["database", "primary", "password"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_MAX_CONNECTIONS",
        path: &["database", "primary", "max_connections"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_MIN_CONNECTIONS",
        path: &["database", "primary", "min_connections"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_ACQUIRE_TIMEOUT_SECS",
        path: &["database", "primary", "acquire_timeout_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_IDLE_TIMEOUT_SECS",
        path: &["database", "primary", "idle_timeout_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_MAX_LIFETIME_SECS",
        path: &["database", "primary", "max_lifetime_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_CONNECT_TIMEOUT_SECS",
        path: &["database", "primary", "connect_timeout_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_REPLICAS",
        path: &["database", "replicas"],
        value_type: EnvValueType::Json,
    },
    EnvOverride {
        name: "APP_DATABASE_SOURCES",
        path: &["database", "sources"],
        value_type: EnvValueType::Json,
    },
    EnvOverride {
        name: "APP_GENERATOR_DATA_SOURCE",
        path: &["generator", "data_source"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_AUTH_JWT_SECRET",
        path: &["auth", "jwt_secret"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_AUTH_ACCESS_TOKEN_EXPIRE",
        path: &["auth", "access_token_expire"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_AUTH_REFRESH_TOKEN_EXPIRE",
        path: &["auth", "refresh_token_expire"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_AUTH_MAX_LOGIN_ATTEMPTS",
        path: &["auth", "max_login_attempts"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_AUTH_LOCKOUT_DURATION_MINUTES",
        path: &["auth", "lockout_duration_minutes"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_REDIS_HOST",
        path: &["redis", "host"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_REDIS_PORT",
        path: &["redis", "port"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_REDIS_PASSWORD",
        path: &["redis", "password"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_REDIS_DATABASE",
        path: &["redis", "database"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_REDIS_MAX_POOL_SIZE",
        path: &["redis", "max_pool_size"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_REDIS_TIMEOUT_SECS",
        path: &["redis", "timeout_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_LOGGER_LEVEL",
        path: &["logger", "level"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_LOGGER_FORMAT",
        path: &["logger", "format"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_LOGGER_OUTPUT",
        path: &["logger", "output"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_CORS_ALLOW_ORIGINS",
        path: &["cors", "allow_origins"],
        value_type: EnvValueType::StringArray,
    },
    EnvOverride {
        name: "APP_RATE_LIMIT_ENABLED",
        path: &["rate_limit", "enabled"],
        value_type: EnvValueType::Bool,
    },
    EnvOverride {
        name: "APP_RATE_LIMIT_CAPACITY",
        path: &["rate_limit", "capacity"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_RATE_LIMIT_REFILL_PER_SEC",
        path: &["rate_limit", "refill_per_sec"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_RATE_LIMIT_WINDOW_SECS",
        path: &["rate_limit", "window_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT",
        path: &["rate_limit", "enable_user_rate_limit"],
        value_type: EnvValueType::Bool,
    },
    EnvOverride {
        name: "APP_RATE_LIMIT_USER_WINDOW_SECS",
        path: &["rate_limit", "user_window_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_RATE_LIMIT_USER_CAPACITY",
        path: &["rate_limit", "user_capacity"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_RATE_LIMIT_API_WINDOW_SECS",
        path: &["rate_limit", "api_window_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_OBJECT_STORAGE_BACKEND",
        path: &["object_storage", "backend"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_OBJECT_STORAGE_LOCAL_BASE_DIR",
        path: &["object_storage", "local_base_dir"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_OBJECT_STORAGE_PUBLIC_BASE_URL",
        path: &["object_storage", "public_base_url"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_OBJECT_STORAGE_ENDPOINT",
        path: &["object_storage", "endpoint"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_OBJECT_STORAGE_ACCESS_KEY",
        path: &["object_storage", "access_key"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_OBJECT_STORAGE_SECRET_KEY",
        path: &["object_storage", "secret_key"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_OBJECT_STORAGE_USE_SSL",
        path: &["object_storage", "use_ssl"],
        value_type: EnvValueType::Bool,
    },
    EnvOverride {
        name: "APP_OBJECT_STORAGE_REGION",
        path: &["object_storage", "region"],
        value_type: EnvValueType::String,
    },
];

fn parse_env_value(name: &str, value: &str, value_type: EnvValueType) -> AppResult<toml::Value> {
    match value_type {
        EnvValueType::String => Ok(toml::Value::String(value.to_string())),
        EnvValueType::Integer => value
            .parse::<i64>()
            .map(toml::Value::Integer)
            .map_err(|e| AppError::Config(format!("环境变量 {} 不是有效整数: {}", name, e))),
        EnvValueType::Bool => value
            .parse::<bool>()
            .map(toml::Value::Boolean)
            .map_err(|e| AppError::Config(format!("环境变量 {} 不是有效布尔值: {}", name, e))),
        EnvValueType::StringArray => {
            let values = value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| toml::Value::String(item.to_string()))
                .collect();
            Ok(toml::Value::Array(values))
        }
        EnvValueType::Json => {
            let json = serde_json::from_str::<serde_json::Value>(value).map_err(|error| {
                AppError::Config(format!("环境变量 {name} 不是有效 JSON: {error}"))
            })?;
            toml::Value::try_from(json).map_err(|error| {
                AppError::Config(format!("环境变量 {name} 无法转换为配置值: {error}"))
            })
        }
    }
}

fn insert_toml_path(table: &mut toml::Table, path: &[&str], value: toml::Value) {
    if path.is_empty() {
        return;
    }

    insert_toml_path_inner(table, path, value);
}

fn insert_toml_path_inner(table: &mut toml::Table, path: &[&str], value: toml::Value) {
    if path.len() == 1 {
        table.insert(path[0].to_string(), value);
        return;
    }

    let child = ensure_table(table, path[0]);
    insert_toml_path_inner(child, &path[1..], value);
}

fn ensure_table<'a>(table: &'a mut toml::Table, key: &str) -> &'a mut toml::Table {
    let value = table
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !value.is_table() {
        *value = toml::Value::Table(toml::Table::new());
    }
    let toml::Value::Table(table) = value else {
        unreachable!("table was initialized above");
    };
    table
}

/// 递归合并两个 TOML Table，env 的值覆盖 base 对应位置的值
///
/// - Table → 递归合并子键
/// - 其他 → env 直接覆盖 base
fn merge_tables(base: &mut toml::Table, env: &toml::Table) {
    for (key, value) in env {
        match (base.get_mut(key), value) {
            // 两地都是 Table → 递归合并
            (Some(toml::Value::Table(base_table)), toml::Value::Table(env_table)) => {
                merge_tables(base_table, env_table);
            }
            // env 覆盖 base
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}
