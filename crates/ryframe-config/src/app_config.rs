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

        // Table → AppConfig
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
    /// 只加载环境配置文件（不加载默认 + 环境变量的完整覆盖），
    /// 返回的 AppConfig 中只填充了可热更新的字段。
    pub fn reload_hot(config_dir: &str) -> AppResult<Self> {
        let env = std::env::var("APP_ENV").unwrap_or_else(|_| "dev".to_string());

        // 读取环境配置文件
        let env_path = format!("{}/app.{}.toml", config_dir, env);
        if let Ok(env_toml) = std::fs::read_to_string(&env_path) {
            let env_table: toml::Table = toml::from_str(&env_toml)
                .map_err(|e| AppError::Config(format!("解析 {} 失败: {}", env_path, e)))?;
            let config: AppConfig = env_table
                .try_into()
                .map_err(|e| AppError::Config(format!("热加载配置反序列化失败: {}", e)))?;
            return Ok(config);
        }

        // 回退到默认配置
        let base_path = format!("{}/app.toml", config_dir);
        let base_toml = std::fs::read_to_string(&base_path)
            .map_err(|e| AppError::Config(format!("无法读取 {}: {}", base_path, e)))?;
        let config: AppConfig = toml::from_str(&base_toml)
            .map_err(|e| AppError::Config(format!("解析 {} 失败: {}", base_path, e)))?;
        Ok(config)
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
