use ryframe_config::{AppConfig, DbConnection, RedisConfig};

#[test]
fn test_load_and_validate_config() {
    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let config = AppConfig::load(config_dir);
    assert!(config.is_ok());
    let cfg = config.unwrap();
    assert_eq!(cfg.app.name, "ryframe");

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
        acquire_timeout_secs: 10,
        idle_timeout_secs: 600,
        max_lifetime_secs: 1800,
        connect_timeout_secs: 10,
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
