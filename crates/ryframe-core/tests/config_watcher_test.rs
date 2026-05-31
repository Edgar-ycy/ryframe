use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, CorsConfig, DatabaseConfig, DbConnection, LoggerConfig,
    RateLimitConfig, RedisConfig, SqlLogLevel,
};
use ryframe_core::HotConfig;

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
        redis: Some(RedisConfig {
            host: "127.0.0.1".into(),
            port: 6379,
            password: "pass".into(),
            database: 0,
            max_pool_size: 5,
            timeout_secs: 3,
        }),
        logger: LoggerConfig {
            level: "info".into(),
            format: "text".into(),
            output: "stdout".into(),
        },
        rate_limit: RateLimitConfig {
            enabled: true,
            capacity: 100,
            refill_per_sec: 10,
            ..Default::default()
        },
        cors: CorsConfig::default(),
        object_storage: Default::default(),
    }
}

#[tokio::test]
async fn test_hot_config_new_and_read() {
    let config = test_config();
    let hot = HotConfig::new(config.clone());

    let current = hot.read().await;
    assert_eq!(current.app.name, "test");
    assert_eq!(current.logger.level, "info");
}

#[tokio::test]
async fn test_hot_config_apply_hot() {
    let config = test_config();
    assert_eq!(config.logger.level, "info");

    let hot = HotConfig::new(config.clone());

    let mut hot_config = test_config();
    hot_config.logger.level = "debug".into();
    hot_config.rate_limit.capacity = 200;
    hot_config.rate_limit.refill_per_sec = 20;
    hot_config.redis.as_mut().unwrap().database = 1;

    hot.apply_hot(&hot_config).await;

    let current = hot.read().await;
    assert_eq!(current.logger.level, "debug");
    assert_eq!(current.rate_limit.capacity, 200);
    assert_eq!(current.rate_limit.refill_per_sec, 20);
    assert_eq!(current.redis.as_ref().unwrap().database, 1);
    assert_eq!(current.app.name, "test");
}

#[test]
fn test_hot_config_arc() {
    let config = test_config();
    let hot = HotConfig::new(config);
    let _arc = hot.arc();
}
