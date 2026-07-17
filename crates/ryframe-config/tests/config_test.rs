use std::sync::Mutex;

use ryframe_config::{
    AppConfig, DatabaseReplicaConfig, DatabaseSourceConfig, DbConnection, RedisConfig,
    StorageBackend,
};

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
    assert_eq!(cfg.object_storage.backend, StorageBackend::Rustfs);
    assert!(cfg.database.replicas.is_empty());
    assert_eq!(cfg.database.sources.len(), 1);
    assert_eq!(cfg.database.sources[0].name, "ryframe_device");
    assert_eq!(
        cfg.database.sources[0].connection.database,
        "ryframe_device"
    );
    assert_eq!(cfg.generator.data_source, "ryframe_device");

    // 空应用名应校验失败
    let mut bad = cfg.clone();
    bad.app.name = "".into();
    assert!(bad.validate("dev").is_err());

    let mut missing_s3_credentials = cfg;
    missing_s3_credentials.object_storage.backend = StorageBackend::S3;
    missing_s3_credentials.object_storage.endpoint.clear();
    assert!(missing_s3_credentials.validate("dev").is_err());
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
    assert_eq!(cfg.database.primary.database, "ryframe_test");
    assert_eq!(cfg.database.primary.driver, "postgres");
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
        std::env::set_var(
            "APP_DATABASE_REPLICAS",
            r#"[{"name":"replica-a","driver":"postgres","host":"replica-a","port":5432,"database":"ryframe","username":"postgres","password":"replica-secret","max_connections":5,"min_connections":1}]"#,
        );
        std::env::set_var(
            "APP_DATABASE_SOURCES",
            r#"[{"name":"ryframe_device","driver":"mysql","host":"device-db","port":3306,"database":"ryframe_device","username":"device","password":"device-secret","max_connections":5,"min_connections":1}]"#,
        );
        std::env::set_var("APP_GENERATOR_DATA_SOURCE", "ryframe_device");
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
    assert_eq!(cfg.database.primary.password, "db-secret-from-env");
    assert_eq!(cfg.database.replicas.len(), 1);
    assert_eq!(cfg.database.replicas[0].name, "replica-a");
    assert_eq!(cfg.database.replicas[0].connection.host, "replica-a");
    assert_eq!(cfg.database.sources.len(), 1);
    assert_eq!(cfg.database.sources[0].connection.host, "device-db");
    assert_eq!(cfg.generator.data_source, "ryframe_device");
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
fn test_database_replica_names_and_drivers_are_validated() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let mut cfg = AppConfig::load(config_dir).unwrap();
    cfg.database.replicas = vec![
        DatabaseReplicaConfig {
            name: "replica-a".into(),
            connection: cfg.database.primary.clone(),
        },
        DatabaseReplicaConfig {
            name: "replica-a".into(),
            connection: cfg.database.primary.clone(),
        },
    ];
    assert!(cfg.validate("dev").is_err());

    cfg.database.replicas[1].name = "replica-b".into();
    cfg.database.replicas[1].connection.driver = "postgres".into();
    assert!(cfg.validate("dev").is_err());
}

#[test]
fn test_named_sources_and_generator_selection_are_validated() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let mut cfg = AppConfig::load(config_dir).unwrap();
    let source = cfg.database.sources[0].clone();

    cfg.database.sources.push(source.clone());
    assert!(cfg.validate("dev").is_err());

    cfg.database.sources.pop();
    cfg.database.sources[0].name = "primary".into();
    assert!(cfg.validate("dev").is_err());

    cfg.database.sources[0] = DatabaseSourceConfig {
        name: "ryframe_device".into(),
        connection: source.connection,
    };
    cfg.generator.data_source = "missing".into();
    assert!(cfg.validate("dev").is_err());
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
            "APP_DATABASE_REPLICAS",
            "APP_DATABASE_SOURCES",
            "APP_GENERATOR_DATA_SOURCE",
            "APP_OBJECT_STORAGE_ACCESS_KEY",
            "APP_OBJECT_STORAGE_SECRET_KEY",
            "APP_RATE_LIMIT_ENABLED",
            "APP_CORS_ALLOW_ORIGINS",
        ] {
            std::env::remove_var(key);
        }
    }
}
