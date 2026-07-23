use std::sync::Mutex;

use ryframe_config::{
    AppConfig, DatabaseReplicaConfig, DatabaseSourceConfig, DbConnection, RedisConfig, RedisMode,
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
    assert_ne!(cfg.database.sources[0].name, "primary");
    assert_eq!(cfg.generator.data_source, cfg.database.sources[0].name);

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
fn test_static_load_uses_full_merged_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();

    unsafe {
        std::env::set_var("APP_ENV", "test");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let cfg = AppConfig::load(config_dir).unwrap();

    assert_eq!(cfg.app.name, "ryframe");
    assert_eq!(cfg.database.primary.database, "ryframe_test");
    assert_eq!(cfg.database.primary.port, 3306);
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
        std::env::set_var(
            "APP_AUTH_JWT_SECRET",
            "prod-secret-from-env-at-least-32-bytes",
        );
        std::env::set_var("APP_DATABASE_PASSWORD", "db-secret-from-env");
        std::env::set_var(
            "APP_DATABASE_REPLICAS",
            r#"[{"name":"replica-a","host":"replica-a","port":3306,"database":"ryframe","username":"root","password":"replica-secret","max_connections":5,"min_connections":1}]"#,
        );
        std::env::set_var(
            "APP_DATABASE_SOURCES",
            r#"[{"name":"reporting","host":"reporting-db","port":3306,"database":"reporting_data","username":"reporting","password":"reporting-secret","max_connections":5,"min_connections":1}]"#,
        );
        std::env::set_var("APP_GENERATOR_DATA_SOURCE", "reporting");
        std::env::set_var("APP_OBJECT_STORAGE_ACCESS_KEY", "object-access");
        std::env::set_var("APP_OBJECT_STORAGE_SECRET_KEY", "object-secret");
        std::env::set_var("APP_RATE_LIMIT_ENABLED", "false");
        std::env::set_var("SNOWFLAKE_WORKER_ID", "17");
        std::env::set_var(
            "APP_CORS_ALLOW_ORIGINS",
            "https://admin.example.com,https://api.example.com",
        );
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let cfg = AppConfig::load(config_dir).unwrap();

    assert_eq!(
        cfg.auth.jwt_secret,
        "prod-secret-from-env-at-least-32-bytes"
    );
    assert_eq!(cfg.database.primary.password, "db-secret-from-env");
    assert_eq!(cfg.database.replicas.len(), 1);
    assert_eq!(cfg.database.replicas[0].name, "replica-a");
    assert_eq!(cfg.database.replicas[0].connection.host, "replica-a");
    assert_eq!(cfg.database.sources.len(), 1);
    assert_eq!(cfg.database.sources[0].connection.host, "reporting-db");
    assert_eq!(cfg.generator.data_source, "reporting");
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
fn test_database_replica_names_are_validated() {
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
    assert!(cfg.validate("dev").is_ok());
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
        name: "business".into(),
        connection: source.connection,
    };
    cfg.generator.data_source = "missing".into();
    assert!(cfg.validate("dev").is_err());
}

#[test]
fn test_connection_urls() {
    let conn = DbConnection {
        host: "db.example.com".into(),
        port: 3306,
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
        "mysql://admin:secret@db.example.com:3306/myapp?collation=utf8mb4_general_ci"
    );

    let redis = RedisConfig {
        mode: RedisMode::Optional,
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

#[test]
fn production_alias_rejects_default_jwt_secret() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("APP_ENV", "production");
        std::env::set_var("SNOWFLAKE_WORKER_ID", "17");
    }
    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    assert!(AppConfig::load(config_dir).is_err());
    clear_config_env();
}

#[test]
fn production_rejects_short_jwt_secret() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("APP_ENV", "prod");
        std::env::set_var("APP_AUTH_JWT_SECRET", "too-short");
        std::env::set_var("SNOWFLAKE_WORKER_ID", "17");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let error = AppConfig::load(config_dir).unwrap_err().to_string();
    assert!(
        error.contains("至少需要 32 字节"),
        "unexpected error: {error}"
    );
    clear_config_env();
}

#[test]
fn validation_rejects_whitespace_only_jwt_secret() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("SNOWFLAKE_WORKER_ID", "17");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let mut config = AppConfig::load(config_dir).unwrap();
    config.auth.jwt_secret = " ".repeat(64);
    let error = config.validate("prod").unwrap_err().to_string();
    assert!(error.contains("不能为空"), "unexpected error: {error}");
    clear_config_env();
}

#[test]
fn production_validation_rejects_space_padded_default_jwt_secret() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("SNOWFLAKE_WORKER_ID", "17");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let mut config = AppConfig::load(config_dir).unwrap();
    config.auth.jwt_secret = format!(
        "{}change-me-in-production{}",
        " ".repeat(16),
        " ".repeat(16)
    );
    let error = config.validate("prod").unwrap_err().to_string();
    assert!(
        error.contains("不允许使用默认值"),
        "unexpected error: {error}"
    );
    clear_config_env();
}

#[test]
fn direct_validation_applies_production_alias_security_rules() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("SNOWFLAKE_WORKER_ID", "17");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let mut cfg = AppConfig::load(config_dir).unwrap();
    cfg.auth.jwt_secret = "short-but-not-the-default".into();
    let error = cfg.validate("production").unwrap_err().to_string();
    assert!(
        error.contains("至少需要 32 字节"),
        "unexpected error: {error}"
    );
    clear_config_env();
}

#[test]
fn login_lockout_policy_must_be_non_zero() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let mut cfg = AppConfig::load(config_dir).unwrap();
    cfg.auth.max_login_attempts = 0;
    assert!(cfg.validate("dev").is_err());

    cfg.auth.max_login_attempts = 5;
    cfg.auth.lockout_duration_minutes = 0;
    assert!(cfg.validate("dev").is_err());
}

#[test]
fn production_requires_explicit_snowflake_worker_id() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("APP_ENV", "prod");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let error = AppConfig::load(config_dir).unwrap_err().to_string();
    assert!(
        error.contains("SNOWFLAKE_WORKER_ID"),
        "unexpected error: {error}"
    );
    clear_config_env();
}

#[test]
fn configured_snowflake_worker_id_is_validated_in_all_environments() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("SNOWFLAKE_WORKER_ID", "1024");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let error = AppConfig::load(config_dir).unwrap_err().to_string();
    assert!(
        error.contains("SNOWFLAKE_WORKER_ID") || error.contains("工作机器 ID"),
        "unexpected error: {error}"
    );
    clear_config_env();
}

#[test]
fn encrypted_value_without_master_key_is_rejected() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("APP_AUTH_JWT_SECRET", "ENC[placeholder]");
    }
    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    assert!(AppConfig::load(config_dir).is_err());
    clear_config_env();
}

#[test]
fn removed_database_driver_field_is_rejected() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var(
            "APP_DATABASE_REPLICAS",
            r#"[{"name":"legacy","driver":"mysql","host":"127.0.0.1","port":3306,"database":"ryframe","username":"root","password":"","max_connections":5,"min_connections":1}]"#,
        );
    }
    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    assert!(AppConfig::load(config_dir).is_err());
    clear_config_env();
}

#[test]
fn clear_config_env_removes_all_app_overrides() {
    let _guard = ENV_LOCK.lock().unwrap();
    unsafe {
        std::env::set_var("APP_DATABASE_PORT", "13306");
        std::env::set_var("APP_OBJECT_STORAGE_BACKEND", "rustfs");
        std::env::set_var("APP_FUTURE_CONFIG_OVERRIDE", "must-be-cleared");
        std::env::set_var("CONFIG_MASTER_KEY", "test-master-key");
    }

    clear_config_env();

    assert!(std::env::var_os("APP_DATABASE_PORT").is_none());
    assert!(std::env::var_os("APP_OBJECT_STORAGE_BACKEND").is_none());
    assert!(std::env::var_os("APP_FUTURE_CONFIG_OVERRIDE").is_none());
    assert!(std::env::var_os("CONFIG_MASTER_KEY").is_none());
    assert!(std::env::var_os("SNOWFLAKE_WORKER_ID").is_none());
}

fn clear_config_env() {
    let keys = std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| {
            let key = key.to_string_lossy();
            key.starts_with("APP_") || key == "CONFIG_MASTER_KEY" || key == "SNOWFLAKE_WORKER_ID"
        })
        .collect::<Vec<_>>();

    unsafe {
        for key in keys {
            std::env::remove_var(key);
        }
    }
}
