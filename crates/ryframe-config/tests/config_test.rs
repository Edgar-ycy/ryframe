use std::sync::Mutex;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use ryframe_config::{
    AppConfig, AuthConfig, ConfigCrypto, DatabaseReplicaConfig, DatabaseSourceConfig, DbConnection,
    DbTlsMode, GeneratorConfig, JobWorkerMode, LoggerConfig, MigrationMode, RateLimitConfig,
    RedisConfig, RedisMode, StorageBackend,
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
    assert_eq!(
        cfg.rate_limit
            .api_limits
            .get("POST /api/v1/auth/password-reset/complete"),
        Some(&3)
    );
    assert_eq!(cfg.pagination.default_page_size, 10);
    assert_eq!(cfg.pagination.max_page_size, 100);
    assert_eq!(cfg.pagination.unpaged_max_records, 1_000);
    assert_eq!(cfg.database.migration_mode, MigrationMode::Auto);
    assert_eq!(cfg.jobs.mode, JobWorkerMode::Embedded);

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
fn pagination_policy_is_overridable_and_validated() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("APP_PAGINATION_DEFAULT_PAGE_SIZE", "25");
        std::env::set_var("APP_PAGINATION_MAX_PAGE_SIZE", "50");
        std::env::set_var("APP_PAGINATION_UNPAGED_MAX_RECORDS", "500");
    }

    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let mut cfg = AppConfig::load(config_dir).unwrap();
    assert_eq!(cfg.pagination.default_page_size, 25);
    assert_eq!(cfg.pagination.max_page_size, 50);
    assert_eq!(cfg.pagination.unpaged_max_records, 500);

    cfg.pagination.default_page_size = 51;
    assert!(cfg.validate("dev").is_err());
    cfg.pagination.default_page_size = 25;
    cfg.pagination.unpaged_max_records = 10_001;
    assert!(cfg.validate("dev").is_err());
    clear_config_env();
}

#[test]
fn migration_mode_defaults_by_environment_and_rejects_unsafe_production_values() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");

    unsafe {
        std::env::set_var("APP_ENV", "prod");
        std::env::set_var(
            "APP_AUTH_JWT_SECRET",
            "prod-secret-from-env-at-least-32-bytes",
        );
        std::env::set_var("APP_DATABASE_PASSWORD", "db-secret");
        std::env::set_var("APP_OBJECT_STORAGE_ALLOW_LOCAL_IN_PRODUCTION", "true");
        std::env::set_var(
            "APP_MONITOR_METRICS_BEARER_TOKEN",
            "metrics-token-for-production-tests-32-bytes",
        );
        std::env::set_var("SNOWFLAKE_WORKER_ID", "18");
    }
    let config = AppConfig::load(config_dir).unwrap();
    assert_eq!(config.database.migration_mode, MigrationMode::Verify);
    assert_eq!(config.jobs.mode, JobWorkerMode::External);

    unsafe {
        std::env::set_var("APP_DATABASE_MIGRATION_MODE", "auto");
    }
    assert!(AppConfig::load(config_dir).is_err());

    unsafe {
        std::env::remove_var("APP_DATABASE_MIGRATION_MODE");
        std::env::set_var("APP_JOBS_MODE", "embedded");
    }
    assert!(AppConfig::load(config_dir).is_err());

    clear_config_env();
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
            r#"[{"name":"replica-a","host":"replica-a","port":3306,"database":"ryframe","username":"root","password":"replica-secret","max_connections":5,"min_connections":1,"tls_mode":"verify_identity","tls_ca":"certs/mysql-ca.pem"}]"#,
        );
        std::env::set_var(
            "APP_DATABASE_SOURCES",
            r#"[{"name":"reporting","host":"reporting-db","port":3306,"database":"reporting_data","username":"reporting","password":"reporting-secret","max_connections":5,"min_connections":1,"tls_mode":"verify_identity","tls_ca":"certs/mysql-ca.pem"}]"#,
        );
        std::env::set_var("APP_GENERATOR_DATA_SOURCE", "reporting");
        std::env::set_var("APP_OBJECT_STORAGE_ACCESS_KEY", "object-access");
        std::env::set_var("APP_OBJECT_STORAGE_SECRET_KEY", "object-secret");
        std::env::set_var("APP_OBJECT_STORAGE_ALLOW_LOCAL_IN_PRODUCTION", "true");
        std::env::set_var(
            "APP_MONITOR_METRICS_BEARER_TOKEN",
            "metrics-token-for-production-tests-32-bytes",
        );
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
        ..DbConnection::default()
    };
    assert_eq!(
        conn.connection_url(),
        "mysql://admin:secret@db.example.com:3306/myapp?collation=utf8mb4_general_ci&ssl-mode=disabled"
    );

    let redis = RedisConfig {
        mode: RedisMode::Optional,
        host: "cache.example.com".into(),
        port: 6380,
        password: "redispass".into(),
        database: 1,
        max_pool_size: 10,
        timeout_secs: 5,
        ..RedisConfig::default()
    };
    assert_eq!(
        redis.connection_url(),
        "redis://:redispass@cache.example.com:6380/1"
    );
}

#[test]
fn nested_configuration_rejects_unknown_fields() {
    assert!(
        toml::from_str::<AuthConfig>(
            r#"
            jwt_secret = "secret"
            access_token_expire = "1h"
            refresh_token_expire = "24h"
            max_login_atempts = 5
            "#,
        )
        .is_err()
    );
    assert!(toml::from_str::<RateLimitConfig>("capcity = 100").is_err());
    assert!(
        toml::from_str::<LoggerConfig>(
            r#"
            level = "info"
            format = "json"
            output = "stdout"
            formt = "text"
            "#,
        )
        .is_err()
    );
    assert!(
        toml::from_str::<GeneratorConfig>(
            r#"
            data_source = "primary"
            data_sorce = "typo"
            "#,
        )
        .is_err()
    );
}

#[test]
fn production_hardening_requires_secure_remote_dependencies_and_explicit_exposure() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("APP_ENV", "dev");
        std::env::set_var("SNOWFLAKE_WORKER_ID", "27");
    }
    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    let mut config = AppConfig::load(config_dir).unwrap();
    config.auth.jwt_secret = "production-secret-with-at-least-32-bytes".into();
    config.cors.allow_origins = vec!["https://admin.example.com".into()];
    config.api_docs.enabled = false;
    config.monitor.metrics_bearer_token = "metrics-secret-with-at-least-32-bytes".into();
    config.redis.as_mut().unwrap().mode = RedisMode::Required;

    config.api_docs.enabled = true;
    assert!(config.validate("prod").is_err());
    config.api_docs.enabled = false;

    config.monitor.metrics_bearer_token.clear();
    assert!(config.validate("prod").is_err());
    config.monitor.metrics_bearer_token = "metrics-secret-with-at-least-32-bytes".into();

    config.database.primary.host = "db.example.com".into();
    assert!(config.validate("prod").is_err());
    config.database.primary.tls_mode = DbTlsMode::VerifyIdentity;
    config.database.primary.tls_ca = Some("certs/mysql-ca.pem".into());

    let redis = config.redis.as_mut().unwrap();
    redis.host = "redis.example.com".into();
    assert!(config.validate("prod").is_err());
    config.redis.as_mut().unwrap().tls = true;
    assert!(config.validate("prod").is_err());

    config.object_storage.use_ssl = true;
    assert!(config.validate("prod").is_err());
    config.object_storage.endpoint = "https://storage.example.com".into();
    assert!(config.validate("prod").is_ok());

    config.object_storage.endpoint = "http://storage.example.com".into();
    assert!(config.validate("prod").is_err());
    config.object_storage.endpoint = "https://storage.example.com".into();

    config.object_storage.backend = StorageBackend::Local;
    config.object_storage.allow_local_in_production = false;
    assert!(config.validate("prod").is_err());
    config.object_storage.allow_local_in_production = true;
    assert!(config.validate("prod").is_ok());

    clear_config_env();
}

#[test]
fn encrypted_metrics_bearer_token_requires_a_master_key_and_is_decrypted() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    let key = [7_u8; 32];
    let plaintext = "metrics-token-for-encryption-tests-32-bytes";
    let encrypted = ConfigCrypto::encrypt(&key, plaintext).expect("加密监控令牌");
    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");

    unsafe {
        std::env::set_var("APP_ENV", "dev");
        std::env::set_var("APP_MONITOR_METRICS_BEARER_TOKEN", &encrypted);
    }
    let error = AppConfig::load(config_dir)
        .expect_err("加密监控令牌必须要求主密钥")
        .to_string();
    assert!(error.contains("CONFIG_MASTER_KEY"));

    unsafe {
        std::env::set_var("CONFIG_MASTER_KEY", BASE64.encode(key));
    }
    let config = AppConfig::load(config_dir).expect("解密监控令牌");
    assert_eq!(config.monitor.metrics_bearer_token, plaintext);
    clear_config_env();
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

#[test]
fn load_from_env_uses_config_directory_override() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    let config_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config");
    unsafe {
        std::env::set_var("APP_CONFIG_DIR", config_dir);
    }

    let config = AppConfig::load_from_env().expect("config directory override should load");
    assert_eq!(config.app.name, "ryframe");
    clear_config_env();
}

#[test]
fn load_from_env_rejects_empty_config_directory() {
    let _guard = ENV_LOCK.lock().unwrap();
    clear_config_env();
    unsafe {
        std::env::set_var("APP_CONFIG_DIR", "   ");
    }

    let error = AppConfig::load_from_env().unwrap_err().to_string();
    assert!(error.contains("APP_CONFIG_DIR"));
    clear_config_env();
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
