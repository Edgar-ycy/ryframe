use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, LoggerConfig,
    RateLimitConfig, SqlLogLevel,
};
use ryframe_core::AppContext;

fn test_config() -> AppConfig {
    AppConfig {
        app: AppSettings {
            name: "test".into(),
            version: "0.1.0".into(),
            host: "127.0.0.1".into(),
            port: 8080,
        },
        database: DatabaseConfig {
            connections: vec![DbConnection {
                driver: "sqlite".into(),
                host: "".into(),
                port: 0,
                database: ":memory:".into(),
                username: "".into(),
                password: "".into(),
                max_connections: 5,
                min_connections: 1,
                acquire_timeout_secs: 10,
                idle_timeout_secs: 600,
                max_lifetime_secs: 1800,
                connect_timeout_secs: 10,
            }],
            sql_log_level: SqlLogLevel::Off,
        },
        auth: AuthConfig {
            jwt_secret: "test-secret".into(),
            access_token_expire: "1h".into(),
            refresh_token_expire: "168h".into(),
            max_login_attempts: 5,
            lockout_duration_minutes: 30,
            enable_password_complexity: true,
        },
        redis: None,
        logger: LoggerConfig {
            level: "info".into(),
            format: "text".into(),
            output: "stdout".into(),
        },
        rate_limit: RateLimitConfig::default(),
        cors: Default::default(),
        object_storage: Default::default(),
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
