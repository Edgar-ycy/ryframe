mod app_config;
mod auth_config;
mod cors_config;
mod db_config;
mod logger_config;
mod rate_limit_config;
mod redis_config;

pub use app_config::AppSettings;
pub use auth_config::AuthConfig;
pub use cors_config::CorsConfig;
pub use db_config::{DatabaseConfig, DbConnection, NamedDataSource, SqlLogLevel};
pub use logger_config::LoggerConfig;
pub use rate_limit_config::RateLimitConfig;
pub use redis_config::RedisConfig;

pub use crate::app_config::AppConfig;

/// 应用 APP_ 前缀环境变量覆盖
///
/// 环境变量命名规则：`APP_` + 配置路径（大写+下划线）
/// 示例：`APP_DATABASE_PRIMARY_HOST` → `database.primary.host`
pub(crate) fn apply_env_overrides(config: &mut AppConfig) {
    for (key, value) in std::env::vars() {
        if !key.starts_with("APP_") || key == "APP_ENV" {
            continue;
        }

        // APP_DATABASE_PRIMARY_HOST → DATABASE_PRIMARY_HOST → database.primary.host
        let path = key[4..].to_lowercase(); // 去掉 "APP_" 前缀，转小写
        let segments: Vec<&str> = path.split('_').collect();

        match segments.as_slice() {
            ["app", "name"] => config.app.name = value,
            ["app", "host"] => config.app.host = value,
            ["app", "port"] => {
                if let Ok(p) = value.parse() {
                    config.app.port = p;
                }
            }
            ["database", "primary", "host"] => config.database.primary.host = value,
            ["database", "primary", "port"] => {
                if let Ok(p) = value.parse() {
                    config.database.primary.port = p;
                }
            }
            ["database", "primary", "database"] => config.database.primary.database = value,
            ["database", "primary", "username"] => config.database.primary.username = value,
            ["database", "primary", "password"] => config.database.primary.password = value,
            ["auth", "jwt_secret"] => config.auth.jwt_secret = value,
            ["auth", "access_token_expire"] => config.auth.access_token_expire = value,
            ["auth", "refresh_token_expire"] => config.auth.refresh_token_expire = value,
            ["redis", "host"] => {
                config.redis.get_or_insert_with(Default::default).host = value;
            }
            ["redis", "port"] => {
                if let Ok(p) = value.parse() {
                    config.redis.get_or_insert_with(Default::default).port = p;
                }
            }
            ["redis", "password"] => {
                config.redis.get_or_insert_with(Default::default).password = value;
            }
            ["logger", "level"] => config.logger.level = value,
            ["logger", "format"] => config.logger.format = value,
            ["logger", "output"] => config.logger.output = value,
            ["cors", "allow_origins"] => {
                let origins: Vec<String> = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                config.cors.allow_origins = origins;
            }
            _ => {} // 未知的环境变量忽略
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_and_validate_config() {
        let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
        let config = AppConfig::load(config_dir);
        assert!(config.is_ok());
        let cfg = config.unwrap();
        assert_eq!(cfg.app.name, "ryframe");
        assert_eq!(cfg.app.port, 8080);

        // 空应用名应校验失败
        let mut bad = cfg;
        bad.app.name = "".into();
        assert!(bad.validate("dev").is_err());
    }

    #[test]
    fn test_connection_urls() {
        let conn = DbConnection {
            driver: "postgres".into(),
            host: "db.example.com".into(),
            port: 5432,
            database: "myapp".into(),
            username: "admin".into(),
            password: "secret".into(),
            max_connections: 10,
            min_connections: 2,
        };
        assert_eq!(
            conn.connection_url(),
            "postgres://admin:secret@db.example.com:5432/myapp"
        );

        let redis = RedisConfig {
            host: "cache.example.com".into(),
            port: 6380,
            password: "redispass".into(),
            database: 1,
            max_pool_size: 10,
            timeout_secs: 5,
        };
        assert_eq!(
            redis.connection_url(),
            "redis://:redispass@cache.example.com:6380/1"
        );
    }
}
