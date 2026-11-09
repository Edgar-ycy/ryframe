use super::super::tls::configuration::Published;
use super::*;
use std::{fs, path::Path};

use super::test_fixture as fixture;
use fixture::{SECRET, fixture, source};

fn write_input(root: &Path, source: &Path) {
    let directory = root.join(".local-tests/integration");
    fs::create_dir_all(&directory).unwrap();
    let openssl = root.join("openssl-fixture");
    fs::write(&openssl, b"offline-only-no-execution").unwrap();
    let content = format!(
        "format_version=1\nrun_id='offline-1'\ntimeout_secs=30\nartifact_root={}\nredis_config_dir={}\nopenssl_program={}\n",
        serde_json::to_string(&root.join("artifacts")).unwrap(),
        serde_json::to_string(source).unwrap(),
        serde_json::to_string(&openssl).unwrap()
    );
    fs::write(directory.join("ci.toml"), content).unwrap();
}

#[test]
fn ci_file_is_required_strict_and_drift_is_rejected() {
    let scope = fixture();
    assert!(IntegrationConfiguration::load(scope.path()).is_err());
    let source = source(scope.path(), true);
    write_input(scope.path(), &source);
    let input = IntegrationConfiguration::load(scope.path()).unwrap();
    assert_eq!(input.run_id, "offline-1");
    assert_eq!(input.timeout.as_secs(), 30);
    let path = scope.path().join(".local-tests/integration/ci.toml");
    let before = fs::read(&path).unwrap();
    let mut changed = before.clone();
    changed.extend_from_slice(format!("\nunknown='{SECRET}'\n").as_bytes());
    fs::write(&path, changed).unwrap();
    let error = IntegrationConfiguration::load(scope.path()).err().unwrap();
    assert!(
        error.to_string().contains(path.to_string_lossy().as_ref())
            && error.to_string().contains("存在未允许字段")
            && !error.to_string().contains(SECRET)
    );
    assert!(input.verify().is_err());
    fs::write(&path, before).unwrap();
    let mut table: toml::Table = files::parse(&fs::read(&path).unwrap(), &path).unwrap();
    table.insert("timeout_secs".into(), 0_i64.into());
    fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
    assert!(
        IntegrationConfiguration::load(scope.path())
            .err()
            .unwrap()
            .to_string()
            .contains("timeout_secs")
    );
}

#[test]
fn fixed_schema_defaults_relative_paths_and_explicit_path_program_are_resolved() {
    let scope = fixture();
    let source = source(scope.path(), true);
    write_input(scope.path(), &source);
    let path = scope.path().join(".local-tests/integration/ci.toml");
    let mut table: toml::Table = files::parse(&fs::read(&path).unwrap(), &path).unwrap();
    table.remove("timeout_secs");
    table.insert("artifact_root".into(), "artifacts".into());
    table.insert("redis_config_dir".into(), "source".into());
    table.insert("openssl_program".into(), "openssl-fixture".into());
    fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
    let input = IntegrationConfiguration::load(scope.path()).unwrap();
    assert_eq!(input.test_jobs, 4);
    assert_eq!(input.timeout.as_secs(), 900);
    assert_eq!(input.artifact_root, scope.path().join("artifacts"));
    assert_eq!(input.redis_config_dir, source);
    assert_eq!(input.openssl_program, scope.path().join("openssl-fixture"));
    table.insert("openssl_program".into(), "openssl".into());
    fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
    let input = IntegrationConfiguration::load(scope.path()).unwrap();
    assert_eq!(
        input.openssl_program.as_os_str(),
        std::ffi::OsStr::new("openssl")
    );
}

#[test]
fn version_run_id_jobs_timeout_and_program_control_characters_are_rejected() {
    let scope = fixture();
    let source = source(scope.path(), true);
    write_input(scope.path(), &source);
    let path = scope.path().join(".local-tests/integration/ci.toml");
    let original: toml::Table = files::parse(&fs::read(&path).unwrap(), &path).unwrap();
    for (field, value) in [
        ("format_version", 2_i64.into()),
        ("run_id", "bad_id".into()),
        ("run_id", "".into()),
        ("test_jobs", 0_i64.into()),
        ("test_jobs", 65_i64.into()),
        ("timeout_secs", 3601_i64.into()),
        ("openssl_program", "".into()),
        ("openssl_program", "bad\nprogram".into()),
    ] {
        let mut table = original.clone();
        table.insert(field.into(), value);
        fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
        let error = IntegrationConfiguration::load(scope.path())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains(field) && error.contains(path.to_string_lossy().as_ref()));
        assert!(
            !error.contains(SECRET) && !error.contains("bad_id") && !error.contains("bad\nprogram")
        );
    }
    let mut table = original;
    table.insert("test_jobs".into(), 2_i64.into());
    fs::write(&path, toml::to_string(&table).unwrap()).unwrap();
    #[cfg(windows)]
    assert!(
        IntegrationConfiguration::load(scope.path())
            .err()
            .unwrap()
            .to_string()
            .contains("Windows必须为4")
    );
    #[cfg(not(windows))]
    assert_eq!(
        IntegrationConfiguration::load(scope.path())
            .unwrap()
            .test_jobs,
        2
    );
}

#[test]
fn tls_upstream_is_rejected_without_downgrade() {
    let scope = fixture();
    let source = source(scope.path(), true);
    write_input(scope.path(), &source);
    fs::write(source.join("app.test.toml"), "[redis]\ntls=true\n").unwrap();
    let error = IntegrationConfiguration::load(scope.path()).err().unwrap();
    assert!(
        error.to_string().contains("redis.tls")
            && error.to_string().contains("只支持普通TCP")
            && error.to_string().contains("不会降级")
    );
    assert!(!error.to_string().contains(SECRET));
    fs::write(
        source.join("app.test.toml"),
        "[redis]\nhost='redis.shared.example'\n",
    )
    .unwrap();
    let error = IntegrationConfiguration::load(scope.path()).err().unwrap();
    assert!(error.to_string().contains("redis.host") && error.to_string().contains("回环"));
    fs::write(source.join("app.test.toml"), "# selected\n").unwrap();
    fs::write(
        source.join("app.toml"),
        fixture::BASE.replace("port = 16379\n", ""),
    )
    .unwrap();
    let error = IntegrationConfiguration::load(scope.path()).err().unwrap();
    assert!(
        error.to_string().contains("redis.host/redis.port")
            && error.to_string().contains("不采用默认6379")
    );
}

#[test]
fn three_role_files_bind_endpoints_ca_and_credentials_and_preserve_source() {
    let scope = fixture();
    let source = source(scope.path(), true);
    write_input(scope.path(), &source);
    let before = fs::read(source.join("app.toml")).unwrap();
    let input = IntegrationConfiguration::load(scope.path()).unwrap();
    let ca = scope.path().join("ca.pem");
    fs::write(&ca, b"offline-ca").unwrap();
    let mut published = Published::new();
    published
        .publish(scope.path(), &input, &ca, 41001, 41002)
        .unwrap();
    let parent = scope.path().join(".local-tests/service-tests");
    let redis = capture::Captured::load(&parent.join("redis-tls/config"), false).unwrap();
    let configured = redis.app.redis.as_ref().unwrap();
    let original = input.redis.app.redis.as_ref().unwrap();
    assert_eq!(configured.host, "127.0.0.1");
    assert_eq!(configured.port, 41001);
    assert!(configured.tls && configured.tls_ca.as_deref() == ca.to_str());
    assert!(configured.password == original.password);
    assert_eq!(
        (
            configured.database,
            configured.timeout_secs,
            configured.max_pool_size
        ),
        (
            original.database,
            original.timeout_secs,
            original.max_pool_size
        )
    );
    let s3 = capture::Captured::load(&parent.join("s3-https/config"), true).unwrap();
    assert_eq!(s3.app.object_storage.endpoint, "https://127.0.0.1:41002");
    assert_eq!(
        s3.app.object_storage.access_key,
        "ryframe-integration-access"
    );
    assert!(s3.app.object_storage.secret_key == "ryframe-integration-secret");
    assert!(s3.app.object_storage.use_ssl);
    let otlp = capture::Captured::load(&parent.join("otlp-https/config"), true).unwrap();
    assert_eq!(
        otlp.app.telemetry.endpoint,
        "https://127.0.0.1:41002/v1/traces"
    );
    assert!(otlp.app.telemetry.enabled);
    for role in ["s3-https", "otlp-https"] {
        let path = parent.join(role).join("config/test.toml");
        let metadata: toml::Table = files::parse(&fs::read(&path).unwrap(), &path).unwrap();
        assert_eq!(metadata["ca_certificate"].as_str(), ca.to_str());
    }
    assert!(fs::read(source.join("app.toml")).unwrap() == before);
    assert!(published.clean(&input).is_empty());
    for role in ["redis-tls", "s3-https", "otlp-https"] {
        assert!(!parent.join(role).join("config").exists());
    }
    input.verify().unwrap();
}

#[test]
fn preexisting_config_is_not_overwritten_and_partial_publication_is_retained() {
    let scope = fixture();
    let source = source(scope.path(), false);
    write_input(scope.path(), &source);
    let input = IntegrationConfiguration::load(scope.path()).unwrap();
    let ca = scope.path().join("ca.pem");
    fs::write(&ca, b"offline-ca").unwrap();
    let destination = scope
        .path()
        .join(".local-tests/service-tests")
        .join("s3-https")
        .join("config");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("foreign"), SECRET).unwrap();
    let mut published = Published::new();
    let error = published
        .publish(scope.path(), &input, &ca, 41001, 41002)
        .unwrap_err();
    assert!(error.contains(destination.to_string_lossy().as_ref()) && !error.contains(SECRET));
    assert!(!published.clean(&input).is_empty());
    assert!(destination.join("foreign").exists());
    assert!(fs::read(destination.join("foreign")).unwrap() == SECRET.as_bytes());
    assert!(
        scope
            .path()
            .join(".local-tests/service-tests/redis-tls/config/app.test.toml")
            .exists()
    );
}

#[test]
fn drift_or_unknown_file_prevents_group_cleanup() {
    let scope = fixture();
    let source = source(scope.path(), true);
    write_input(scope.path(), &source);
    let input = IntegrationConfiguration::load(scope.path()).unwrap();
    let ca = scope.path().join("ca.pem");
    fs::write(&ca, b"offline-ca").unwrap();
    let mut published = Published::new();
    published
        .publish(scope.path(), &input, &ca, 41001, 41002)
        .unwrap();
    let parent = scope.path().join(".local-tests/service-tests");
    fs::write(parent.join("otlp-https/config/foreign"), SECRET).unwrap();
    assert!(!published.clean(&input).is_empty());
    for role in ["redis-tls", "s3-https", "otlp-https"] {
        assert!(parent.join(role).join("config").exists());
    }
    fs::remove_file(parent.join("otlp-https/config/foreign")).unwrap();
    let mut replacement = Published::new();
    assert!(
        replacement
            .publish(scope.path(), &input, &ca, 41003, 41004)
            .is_err()
    );
    fs::write(source.join("app.test.toml"), format!("changed='{SECRET}'")).unwrap();
    assert!(input.verify().is_err());
    assert!(!replacement.clean(&input).is_empty());
    assert!(parent.join("redis-tls/config").exists());
}
