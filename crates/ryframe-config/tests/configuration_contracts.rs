use ryframe_config::{
    Environment, JobConfig, MAX_EXPORT_ROWS, MAX_XLSX_DATA_ROWS, MigrationMode,
    ObjectStorageConfig, RedisConfig, ResetConfig, ResourceScopeId,
};

const _: () = assert!(MAX_EXPORT_ROWS <= MAX_XLSX_DATA_ROWS);

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
}
