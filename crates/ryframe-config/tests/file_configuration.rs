mod support;

use ryframe_config::{AppConfig, Environment, JobWorkerMode};
use support::{Fixture, PRODUCTION};

#[test]
fn ci_service_files_load_through_the_same_validated_configuration_path() {
    let fixture = Fixture::new();
    fixture.write(
        Environment::Test,
        &format!(
            "scope_id = \"ci-123-1\"\nsnowflake_worker_id = 1\n{}",
            include_str!("../../../tools/fixtures/integration/app.test.toml")
        ),
    );
    let config = fixture.load(Environment::Test);
    assert_eq!(config.scope_id.as_str(), "ci-123-1");
    assert_eq!(config.snowflake_worker_id, 1);
    assert_eq!(
        config.database.primary.tls_mode,
        ryframe_config::DbTlsMode::Required
    );
    assert_eq!(config.database.primary.database, "mysql");
    let redis = config.redis.as_ref().expect("CI 必须显式配置 Redis");
    assert_eq!(redis.mode, ryframe_config::RedisMode::Required);
    assert_eq!(redis.scope_id().as_str(), "ci-123-1");
    assert_eq!(redis.database, 15);
    assert!(!redis.tls);
    assert!(config.bootstrap.admin_password.is_some());
}

#[test]
fn every_selected_environment_file_is_required_and_only_selected_file_is_read() {
    for environment in [Environment::Dev, Environment::Test, Environment::Prod] {
        let fixture = Fixture::new();
        let error = fixture.error(environment);
        assert!(error.contains(&format!("app.{environment}.toml")));
        assert!(error.contains("创建该文件"));
    }
    let fixture = Fixture::new();
    fixture.write(Environment::Dev, "snowflake_worker_id = 9");
    fixture.write(Environment::Test, "malformed TOML and secret-not-for-dev");
    let config = fixture.load(Environment::Dev);
    assert_eq!(config.environment, Environment::Dev);
    assert_eq!(config.snowflake_worker_id, 9);
    assert!(config.bootstrap.admin_password.is_none());
}

#[test]
fn environment_names_remain_exact_without_aliases_or_whitespace() {
    for (name, expected) in [
        ("dev", Environment::Dev),
        ("test", Environment::Test),
        ("prod", Environment::Prod),
    ] {
        assert_eq!(name.parse::<Environment>().unwrap(), expected);
    }
    for invalid in ["", "DEV", "production", " dev", "prod "] {
        assert!(invalid.parse::<Environment>().is_err());
    }
}

#[test]
fn shared_defaults_deep_merge_and_arrays_replace_instead_of_append() {
    let fixture = Fixture::new();
    let base = fixture.directory.join("app.toml");
    let source = std::fs::read_to_string(&base).unwrap();
    std::fs::write(
        &base,
        source.replace(
            "allow_origins = []",
            "allow_origins = ['http://localhost:8000']",
        ),
    )
    .unwrap();
    fixture.write(
        Environment::Test,
        r#"
snowflake_worker_id = 7
[app]
port = 8123
[cors]
allow_origins = ["http://localhost:8123"]
[proxy]
trusted_cidrs = []
"#,
    );
    let config = fixture.load(Environment::Test);
    assert_eq!(config.app.name, "ryframe");
    assert_eq!(config.app.port, 8123);
    assert_eq!(config.cors.allow_origins, ["http://localhost:8123"]);
    assert!(config.proxy.trusted_cidrs.is_empty());
    assert_eq!(config.jobs.mode, JobWorkerMode::Embedded);
}

#[test]
fn production_accepts_plaintext_credentials_and_uses_external_jobs() {
    let fixture = Fixture::new();
    fixture.write(Environment::Prod, PRODUCTION);
    let config = fixture.load(Environment::Prod);
    assert_eq!(
        config.auth.jwt_secret,
        "production-jwt-secret-must-remain-private-123456789"
    );
    assert_eq!(
        config.redis.as_ref().unwrap().password,
        "production-redis-private"
    );
    assert_eq!(
        config.object_storage.secret_key,
        "production-storage-private"
    );
    assert_eq!(config.jobs.mode, JobWorkerMode::External);
    let debug = format!("{config:?}");
    assert!(!debug.contains("private"));
    assert!(!debug.contains("production-jwt-secret"));
}

#[test]
fn production_still_rejects_weak_jwt_and_monitor_tokens_and_embedded_jobs() {
    let fixture = Fixture::new();
    for weak in ["short-private", "change-me-in-production"] {
        fixture.write(
            Environment::Prod,
            &PRODUCTION.replace("production-jwt-secret-must-remain-private-123456789", weak),
        );
        let error = fixture.error(Environment::Prod);
        assert!(error.contains("auth.jwt_secret"));
        assert!(!error.contains(weak));
    }
    fixture.write(
        Environment::Prod,
        &PRODUCTION.replace(
            "production-metrics-token-must-remain-private-123456789",
            "short-monitor-private",
        ),
    );
    assert!(
        fixture
            .error(Environment::Prod)
            .contains("monitor.metrics_bearer_token")
    );
    fixture.write(
        Environment::Prod,
        &format!("{PRODUCTION}\n[jobs]\nmode = 'embedded'\n"),
    );
    assert!(fixture.error(Environment::Prod).contains("jobs.mode"));
}

#[test]
fn worker_id_is_a_required_toml_integer_in_the_existing_range() {
    let fixture = Fixture::new();
    for worker_id in [0, 1023] {
        fixture.write(
            Environment::Dev,
            &format!("snowflake_worker_id = {worker_id}"),
        );
        assert_eq!(
            fixture.load(Environment::Dev).snowflake_worker_id,
            worker_id
        );
    }
    for worker_id in [-1, 1024] {
        fixture.write(
            Environment::Dev,
            &format!("snowflake_worker_id = {worker_id}"),
        );
        assert!(
            fixture
                .error(Environment::Dev)
                .contains("Snowflake worker ID")
        );
    }
    let base = fixture.directory.join("app.toml");
    let source = std::fs::read_to_string(&base).unwrap();
    std::fs::write(&base, source.replace("snowflake_worker_id = 1", "")).unwrap();
    fixture.write(Environment::Dev, "");
    assert!(
        fixture
            .error(Environment::Dev)
            .contains("snowflake_worker_id")
    );
}

#[test]
fn checked_in_dev_and_test_files_load_without_any_injected_configuration() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config");
    for environment in [Environment::Dev, Environment::Test] {
        let config = AppConfig::load(&root, environment).expect("样例应完整有效");
        assert_eq!(config.environment, environment);
    }
}
