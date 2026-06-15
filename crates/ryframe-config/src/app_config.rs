use ryframe_common::{AppError, AppResult};
use serde::Deserialize;

use crate::{
    AuthConfig, CorsConfig, DatabaseConfig, LoggerConfig, ObjectStorageConfig, RateLimitConfig,
    RedisConfig,
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
#[allow(clippy::derivable_impls)]
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
        if self.database.connections.is_empty() {
            return Err(AppError::Config(
                "database.connections 至少需要一个连接".into(),
            ));
        }
        let primary = &self.database.connections[0];
        if primary.host.is_empty() {
            return Err(AppError::Config(
                "database.connections[0].host 不能为空".into(),
            ));
        }
        if primary.database.is_empty() {
            return Err(AppError::Config(
                "database.connections[0].database 不能为空".into(),
            ));
        }
        if self.auth.jwt_secret.is_empty() {
            return Err(AppError::Config("auth.jwt_secret 不能为空".into()));
        }
        if env == "prod" && self.auth.jwt_secret == "change-me-in-production" {
            return Err(AppError::Config(
                "生产环境必须修改 auth.jwt_secret，不允许使用默认值".into(),
            ));
        }
        Ok(())
    }
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
        path: &["database", "connections", "0", "driver"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_HOST",
        path: &["database", "connections", "0", "host"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_PORT",
        path: &["database", "connections", "0", "port"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_NAME",
        path: &["database", "connections", "0", "database"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_USERNAME",
        path: &["database", "connections", "0", "username"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_PASSWORD",
        path: &["database", "connections", "0", "password"],
        value_type: EnvValueType::String,
    },
    EnvOverride {
        name: "APP_DATABASE_MAX_CONNECTIONS",
        path: &["database", "connections", "0", "max_connections"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_MIN_CONNECTIONS",
        path: &["database", "connections", "0", "min_connections"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_ACQUIRE_TIMEOUT_SECS",
        path: &["database", "connections", "0", "acquire_timeout_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_IDLE_TIMEOUT_SECS",
        path: &["database", "connections", "0", "idle_timeout_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_MAX_LIFETIME_SECS",
        path: &["database", "connections", "0", "max_lifetime_secs"],
        value_type: EnvValueType::Integer,
    },
    EnvOverride {
        name: "APP_DATABASE_CONNECT_TIMEOUT_SECS",
        path: &["database", "connections", "0", "connect_timeout_secs"],
        value_type: EnvValueType::Integer,
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
        name: "APP_AUTH_ENABLE_PASSWORD_COMPLEXITY",
        path: &["auth", "enable_password_complexity"],
        value_type: EnvValueType::Bool,
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

    if path.len() >= 3
        && let Ok(index) = path[1].parse::<usize>()
    {
        let child = ensure_array_table(table, path[0], index);
        insert_toml_path_inner(child, &path[2..], value);
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

fn ensure_array_table<'a>(
    table: &'a mut toml::Table,
    key: &str,
    index: usize,
) -> &'a mut toml::Table {
    let array = table
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    if !array.is_array() {
        *array = toml::Value::Array(Vec::new());
    }
    let toml::Value::Array(array) = array else {
        unreachable!("array was initialized above");
    };
    while array.len() <= index {
        array.push(toml::Value::Table(toml::Table::new()));
    }
    if !array[index].is_table() {
        array[index] = toml::Value::Table(toml::Table::new());
    }
    let toml::Value::Table(table) = &mut array[index] else {
        unreachable!("array item was initialized above");
    };
    table
}

/// 递归合并两个 TOML Table，env 的值覆盖 base 对应位置的值
///
/// - Table → 递归合并子键
/// - Array of Tables → 按索引合并（env[i] 覆盖 base[i] 对应字段，env 多出的追加）
/// - 其他 → env 直接覆盖 base
fn merge_tables(base: &mut toml::Table, env: &toml::Table) {
    for (key, value) in env {
        match (base.get_mut(key), value) {
            // 两地都是 Table → 递归合并
            (Some(toml::Value::Table(base_table)), toml::Value::Table(env_table)) => {
                merge_tables(base_table, env_table);
            }
            // 两地都是 Array of Tables → 按索引递归合并
            (Some(toml::Value::Array(base_arr)), toml::Value::Array(env_arr))
                if is_array_of_tables(base_arr) && is_array_of_tables(env_arr) =>
            {
                merge_array_of_tables(base_arr, env_arr);
            }
            // env 覆盖 base
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// 检查数组中所有元素是否都是 Table
fn is_array_of_tables(arr: &[toml::Value]) -> bool {
    arr.iter().all(|v| matches!(v, toml::Value::Table(_)))
}

/// 合并两个 Array of Tables：按索引递归合并 Table，env 多出的追加
fn merge_array_of_tables(base_arr: &mut Vec<toml::Value>, env_arr: &[toml::Value]) {
    let base_len = base_arr.len();
    for (i, env_val) in env_arr.iter().enumerate() {
        if let toml::Value::Table(env_table) = env_val {
            if i < base_len {
                if let toml::Value::Table(base_table) = &mut base_arr[i] {
                    merge_tables(base_table, env_table);
                }
            } else {
                base_arr.push(toml::Value::Table(env_table.clone()));
            }
        }
    }
}
