use std::sync::Mutex;

use ryframe_config::{AppConfig, DbConnection, RedisConfig};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_load_and_validate_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();

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
fn test_reload_hot_uses_full_merged_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();

    unsafe {
        std::env::set_var("APP_ENV", "test");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let cfg = AppConfig::reload_hot(config_dir).unwrap();

    assert_eq!(cfg.app.name, "ryframe");
    assert_eq!(cfg.database.connections[0].database, "ryframe_test");
    assert_eq!(cfg.database.connections[0].driver, "postgres");
    assert_eq!(cfg.auth.access_token_expire, "5m");
    assert_eq!(cfg.logger.level, "debug");

    clear_config_env();
}

#[test]
fn test_env_overrides_are_applied_before_validation() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();

    unsafe {
        std::env::set_var("APP_ENV", "prod");
        std::env::set_var("APP_AUTH_JWT_SECRET", "prod-secret-from-env");
        std::env::set_var("APP_DATABASE_PASSWORD", "db-secret-from-env");
        std::env::set_var("APP_OBJECT_STORAGE_ACCESS_KEY", "object-access");
        std::env::set_var("APP_OBJECT_STORAGE_SECRET_KEY", "object-secret");
        std::env::set_var("APP_RATE_LIMIT_ENABLED", "false");
        std::env::set_var(
            "APP_CORS_ALLOW_ORIGINS",
            "https://admin.example.com,https://api.example.com",
        );
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let cfg = AppConfig::load(config_dir).unwrap();

    assert_eq!(cfg.auth.jwt_secret, "prod-secret-from-env");
    assert_eq!(cfg.database.connections[0].password, "db-secret-from-env");
    assert_eq!(cfg.object_storage.access_key, "object-access");
    assert_eq!(cfg.object_storage.secret_key, "object-secret");
    assert!(!cfg.rate_limit.enabled);
    assert_eq!(
        cfg.cors.allow_origins,
        vec![
            "https://admin.example.com".to_string(),
            "https://api.example.com".to_string(),
        ]
    );

    clear_config_env();
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

fn clear_config_env() {
    unsafe {
        for key in [
            "APP_ENV",
            "APP_AUTH_JWT_SECRET",
            "APP_DATABASE_PASSWORD",
            "APP_OBJECT_STORAGE_ACCESS_KEY",
            "APP_OBJECT_STORAGE_SECRET_KEY",
            "APP_RATE_LIMIT_ENABLED",
            "APP_CORS_ALLOW_ORIGINS",
        ] {
            std::env::remove_var(key);
        }
    }
}
