use ryframe_config::{
    AppConfig, AppSettings, AuthConfig, CorsConfig, DatabaseConfig, DbConnection, LoggerConfig,
    RateLimitConfig, RedisConfig,
};
use ryframe_core::HotConfig;

fn test_config() -> AppConfig {
    AppConfig {
        app: AppSettings {
            name: "test".into(),
            ..Default::default()
        },
        database: DatabaseConfig {
            primary: DbConnection {
                driver: "sqlite".into(),
                database: ":memory:".into(),
                max_connections: 5,
                ..Default::default()
            },
            ..Default::default()
        },
        generator: Default::default(),
        auth: AuthConfig {
            jwt_secret: "test-secret".into(),
            ..Default::default()
        },
        redis: Some(RedisConfig {
            host: "127.0.0.1".into(),
            password: "pass".into(),
            max_pool_size: 5,
            ..Default::default()
        }),
        logger: LoggerConfig {
            level: "info".into(),
            ..Default::default()
        },
        rate_limit: RateLimitConfig {
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
