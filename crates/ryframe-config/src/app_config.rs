use serde::Deserialize;
use ryframe_common::{AppError, AppResult};
use crate::{apply_env_overrides, AuthConfig, CorsConfig, DatabaseConfig, LoggerConfig, RateLimitConfig, RedisConfig};

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
        let base_toml = std::fs::read_to_string(&base_path).map_err(|e| {
            AppError::Config(format!("无法读取 {}: {}", base_path, e))
        })?;
        let mut table: toml::Table = toml::from_str(&base_toml).map_err(|e| {
            AppError::Config(format!("解析 {} 失败: {}", base_path, e))
        })?;

        // 第二层：加载环境配置文件，merge 到 base table
        let env_path = format!("{}/app.{}.toml", config_dir, env);
        if let Ok(env_toml) = std::fs::read_to_string(&env_path) {
            let env_table: toml::Table = toml::from_str(&env_toml).map_err(|e| {
                AppError::Config(format!("解析 {} 失败: {}", env_path, e))
            })?;
            merge_tables(&mut table, &env_table);
        }

        // Table → AppConfig
        let mut config: AppConfig = table.try_into().map_err(|e| {
            AppError::Config(format!("配置反序列化失败: {}", e))
        })?;

        // 第三层：APP_ 前缀环境变量覆盖
        apply_env_overrides(&mut config);

        // 校验
        config.validate(&env)?;

        Ok(config)
    }

    /// 校验必填配置项
    pub(crate) fn validate(&self, env: &str) -> AppResult<()> {
        if self.app.name.is_empty() {
            return Err(AppError::Config("app.name 不能为空".into()));
        }
        if self.app.host.is_empty() {
            return Err(AppError::Config("app.host 不能为空".into()));
        }
        if self.app.port == 0 {
            return Err(AppError::Config("app.port 必须大于 0".into()));
        }
        if self.database.primary.host.is_empty() {
            return Err(AppError::Config("database.primary.host 不能为空".into()));
        }
        if self.database.primary.database.is_empty() {
            return Err(AppError::Config("database.primary.database 不能为空".into()));
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