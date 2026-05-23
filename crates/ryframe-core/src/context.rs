use chrono::{DateTime, Utc};
use ryframe_config::AppConfig;

/// 应用全局上下文
///
/// 在 main.rs 启动时创建，通过 Axum State 注入到所有 Handler 中。
#[derive(Debug, Clone)]
pub struct AppContext {
    /// 应用配置
    pub config: AppConfig,
    /// 服务启动时间
    pub start_time: DateTime<Utc>,
}

impl AppContext {
    /// 创建应用上下文
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            start_time: Utc::now(),
        }
    }

    /// 服务已运行时长
    pub fn uptime(&self) -> chrono::Duration {
        Utc::now() - self.start_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryframe_config::{
        AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, LoggerConfig,
        RateLimitConfig,
    };

    fn test_config() -> AppConfig {
        AppConfig {
            app: AppSettings {
                name: "test".into(),
                version: "0.1.0".into(),
                host: "127.0.0.1".into(),
                port: 8080,
            },
            database: DatabaseConfig {
                primary: DbConnection {
                    driver: "sqlite".into(),
                    host: "".into(),
                    port: 0,
                    database: ":memory:".into(),
                    username: "".into(),
                    password: "".into(),
                    max_connections: 5,
                    min_connections: 1,
                },
                replicas: vec![],
                datasources: vec![],
                sql_log_level: ryframe_config::SqlLogLevel::Off,
            },
            auth: AuthConfig {
                jwt_secret: "test-secret".into(),
                access_token_expire: "1h".into(),
                refresh_token_expire: "168h".into(),
            },
            redis: None,
            logger: LoggerConfig {
                level: "info".into(),
                format: "text".into(),
                output: "stdout".into(),
            },
            rate_limit: RateLimitConfig::default(),
            cors: Default::default(),
        }
    }

    #[test]
    fn test_app_context() {
        let ctx = AppContext::new(test_config());
        assert_eq!(ctx.config.app.name, "test");
        assert!(ctx.uptime().num_milliseconds() >= 0);

        std::thread::sleep(std::time::Duration::from_millis(10));
        assert!(ctx.uptime().num_milliseconds() >= 10);
    }
}
