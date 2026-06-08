use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, DatabaseConfig, DbConnection, LoggerConfig, RateLimitConfig,
};
use ryframe_core::AppContext;

fn test_config() -> AppConfig {
    AppConfig {
        app: AppSettings {
            name: "test".into(),
            ..Default::default()
        },
        database: DatabaseConfig {
            connections: vec![DbConnection {
                driver: "sqlite".into(),
                database: ":memory:".into(),
                max_connections: 5,
                ..Default::default()
            }],
            ..Default::default()
        },
        auth: AuthConfig {
            jwt_secret: "test-secret".into(),
            ..Default::default()
        },
        redis: None,
        logger: LoggerConfig {
            level: "info".into(),
            ..Default::default()
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
