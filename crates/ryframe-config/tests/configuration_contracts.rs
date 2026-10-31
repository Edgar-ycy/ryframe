use ryframe_config::{
    AuthConfig, DatabaseConfig, DatabaseReplicaConfig, DatabaseSourceConfig, DbConnection,
    DbTlsMode, Environment, JobConfig, MAX_EXPORT_ROWS, MAX_XLSX_DATA_ROWS, MigrationMode,
    MonitorConfig, ObjectStorageConfig, RedisConfig, ResetConfig, ResourceScopeId,
};

const _: () = assert!(MAX_EXPORT_ROWS <= MAX_XLSX_DATA_ROWS);

#[test]
fn new_projects_default_to_single_tenant_without_changing_internal_isolation() {
    let mode = ryframe_config::MultiTenancyConfig::default();
    assert!(!mode.enabled);
    assert_eq!(mode.fixed_tenant_id(), Some("system"));
    assert!(mode.allows_tenant("system"));
    assert!(!mode.allows_tenant("other"));
    let config: toml::Value = toml::from_str(include_str!("../../../config/app.toml")).unwrap();
    assert_eq!(config["multi_tenancy"]["enabled"].as_bool(), Some(false));
    let explicit: ryframe_config::MultiTenancyConfig = toml::from_str("enabled = true").unwrap();
    assert!(explicit.allows_tenant("other"));
}

#[test]
fn checked_in_config_keeps_api_window_outside_the_route_map() {
    let config: toml::Value =
        toml::from_str(include_str!("../../../config/app.toml")).expect("默认配置应是有效 TOML");
    let rate_limit = config
        .get("rate_limit")
        .and_then(toml::Value::as_table)
        .expect("默认配置应包含 rate_limit");
    assert_eq!(
        rate_limit
            .get("api_window_secs")
            .and_then(toml::Value::as_integer),
        Some(60)
    );
    assert!(
        !rate_limit
            .get("api_limits")
            .and_then(toml::Value::as_table)
            .expect("默认配置应包含接口限流规则")
            .contains_key("api_window_secs")
    );
}

#[test]
fn runtime_migration_mode_defaults_to_verify_and_rejects_removed_auto() {
    assert_eq!(MigrationMode::default(), MigrationMode::Verify);
    let removed: Result<MigrationMode, _> = toml::Value::String("auto".into()).try_into();
    assert!(removed.is_err());
}

#[test]
fn export_limit_cannot_exceed_business_or_xlsx_bounds() {
    let config = JobConfig {
        export_max_rows: MAX_EXPORT_ROWS,
        ..JobConfig::default()
    };
    config
        .validate(Environment::Dev)
        .expect("业务行数上限应可用");

    let config = JobConfig {
        export_max_rows: MAX_EXPORT_ROWS + 1,
        ..JobConfig::default()
    };
    assert!(config.validate(Environment::Dev).is_err());
}

#[test]
fn production_scope_must_replace_the_placeholder() {
    let placeholder =
        ResourceScopeId::parse("replace-with-unique-scope").expect("占位作用域格式有效");
    let explicit = ResourceScopeId::parse("production-main").expect("生产作用域有效");

    assert!(placeholder.validate_environment(Environment::Prod).is_err());
    assert!(placeholder.validate_environment(Environment::Dev).is_ok());
    assert!(explicit.validate_environment(Environment::Prod).is_ok());
}

#[test]
fn scope_generates_stable_resource_namespaces() {
    let scope = ResourceScopeId::parse("dev_local-01").expect("作用域有效");
    assert_eq!(scope.redis_namespace(), "ryframe:{dev_local-01}:");
    assert_eq!(scope.object_prefix(), "dev_local-01/");
    assert_eq!(
        scope.ownership_marker("redis"),
        "ryframe-owner:v1:dev_local-01:redis"
    );
}

#[test]
fn scope_rejects_ambiguous_or_path_like_values() {
    for invalid in ["a", "Dev", "dev local", "dev/local", "-dev", "dev-"] {
        assert!(ResourceScopeId::parse(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn legacy_exclusive_flags_are_fail_closed_in_production() {
    let config = ResetConfig {
        legacy_mysql_exclusive: true,
        ..ResetConfig::default()
    };
    assert!(config.validate(true).is_err());
    assert!(config.validate(false).is_ok());
}

#[test]
fn outside_sentinel_key_is_bounded() {
    let config = ResetConfig {
        redis_outside_sentinel_key: Some("sentinel:other-scope".into()),
        credential_version: Some("test-v1".into()),
        ..ResetConfig::default()
    };
    assert!(config.validate(false).is_ok());
    let invalid = ResetConfig {
        redis_outside_sentinel_key: Some(" sentinel".into()),
        ..ResetConfig::default()
    };
    assert!(invalid.validate(false).is_err());
}

#[test]
fn connector_debug_output_redacts_credentials() {
    let redis: RedisConfig = toml::from_str(
        r#"
        password = "redis-password-must-not-leak"
        tls_client_key = "redis-client-key-must-not-leak"
        "#,
    )
    .expect("解析 Redis 脱敏测试配置");
    let redis_debug = format!("{redis:?}");
    assert!(!redis_debug.contains("redis-password-must-not-leak"));
    assert!(!redis_debug.contains("redis-client-key-must-not-leak"));
    assert!(redis_debug.contains("<redacted>"));

    let storage = ObjectStorageConfig {
        access_key: "storage-access-must-not-leak".into(),
        secret_key: "storage-secret-must-not-leak".into(),
        ..ObjectStorageConfig::default()
    };
    let storage_debug = format!("{storage:?}");
    assert!(!storage_debug.contains("storage-access-must-not-leak"));
    assert!(!storage_debug.contains("storage-secret-must-not-leak"));
    assert!(storage_debug.contains("<redacted>"));

    let auth = AuthConfig {
        jwt_secret: "jwt-secret-must-not-leak".into(),
        access_token_expire: "30s".into(),
        refresh_token_expire: "90s".into(),
        ..AuthConfig::default()
    };
    for debug in [format!("{auth:?}"), format!("{auth:#?}")] {
        assert!(!debug.contains("jwt-secret-must-not-leak"));
        for diagnostic in [
            "jwt_secret",
            "<redacted>",
            "30s",
            "90s",
            "max_login_attempts",
            "lockout_duration_minutes",
        ] {
            assert!(debug.contains(diagnostic));
        }
    }
    let monitor = MonitorConfig {
        metrics_bearer_token: "metrics-token-must-not-leak".into(),
    };
    for debug in [format!("{monitor:?}"), format!("{monitor:#?}")] {
        assert!(!debug.contains("metrics-token-must-not-leak"));
        assert!(debug.contains("metrics_bearer_token"));
        assert!(debug.contains("<redacted>"));
    }
}

#[test]
fn database_debug_redacts_direct_and_nested_credentials_without_losing_topology() {
    let connection = DbConnection {
        host: "db-debug-host".into(),
        port: 3399,
        database: "debug_database".into(),
        username: "debug_user".into(),
        password: "primary-password-must-not-leak".into(),
        tls_mode: DbTlsMode::VerifyIdentity,
        tls_ca: Some("public-db-ca.pem".into()),
        tls_client_cert: Some("public-db-cert.pem".into()),
        tls_client_key: Some("primary-client-key-must-not-leak".into()),
        ..DbConnection::default()
    };
    let database = DatabaseConfig {
        primary: connection.clone(),
        replicas: vec![DatabaseReplicaConfig {
            name: "debug-replica".into(),
            connection: DbConnection {
                password: "replica-password-must-not-leak".into(),
                tls_client_key: Some("replica-client-key-must-not-leak".into()),
                ..connection.clone()
            },
        }],
        sources: vec![DatabaseSourceConfig {
            name: "debug-source".into(),
            connection: DbConnection {
                password: "source-password-must-not-leak".into(),
                tls_client_key: Some("source-client-key-must-not-leak".into()),
                ..connection.clone()
            },
        }],
        ..DatabaseConfig::default()
    };
    for debug in [
        format!("{connection:?}"),
        format!("{connection:#?}"),
        format!("{database:?}"),
        format!("{database:#?}"),
    ] {
        for secret in [
            "primary-password-must-not-leak",
            "primary-client-key-must-not-leak",
            "replica-password-must-not-leak",
            "replica-client-key-must-not-leak",
            "source-password-must-not-leak",
            "source-client-key-must-not-leak",
        ] {
            assert!(!debug.contains(secret));
        }
        for diagnostic in [
            "db-debug-host",
            "3399",
            "debug_database",
            "debug_user",
            "VerifyIdentity",
            "public-db-ca.pem",
            "public-db-cert.pem",
            "password",
            "tls_client_key",
            "<redacted>",
        ] {
            assert!(debug.contains(diagnostic));
        }
    }
    for debug in [format!("{database:?}"), format!("{database:#?}")] {
        for diagnostic in [
            "primary",
            "replicas",
            "sources",
            "debug-replica",
            "debug-source",
        ] {
            assert!(debug.contains(diagnostic));
        }
    }
}
