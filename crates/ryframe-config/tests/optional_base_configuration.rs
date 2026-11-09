use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use ryframe_config::{AppConfig, Environment, JobWorkerMode};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.local-tests/optional-base-contracts");
        fs::create_dir_all(&parent).unwrap();
        let directory = parent.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }

    fn selected(&self, environment: Environment, source: &str) {
        fs::write(self.0.join(format!("app.{environment}.toml")), source).unwrap();
    }

    fn load(&self, environment: Environment) -> ryframe_kernel::AppResult<AppConfig> {
        let before = snapshot(&self.0);
        let result = AppConfig::load(&self.0, environment);
        assert_eq!(snapshot(&self.0), before, "加载不得改写配置输入");
        result
    }

    fn error(&self, environment: Environment) -> String {
        let error = self.load(environment).unwrap_err().to_string();
        assert!(!error.contains("PRIVATE-SENTINEL"), "{error}");
        error
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    File(Vec<u8>),
    Directory,
    Link(PathBuf),
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            let value = if kind.is_symlink() {
                Entry::Link(fs::read_link(&path).unwrap())
            } else if kind.is_dir() {
                pending.push(path.clone());
                Entry::Directory
            } else {
                Entry::File(fs::read(&path).unwrap())
            };
            result.insert(path.strip_prefix(root).unwrap().to_path_buf(), value);
        }
    }
    result
}

fn complete(environment: Environment) -> toml::Table {
    let mut table: toml::Table = toml::from_str(include_str!("../../../config/app.toml")).unwrap();
    if environment == Environment::Prod {
        let overlay = toml::from_str::<toml::Table>(
            r#"
scope_id = "optional-base-production"
[auth]
jwt_secret = "PRIVATE-SENTINEL-production-jwt-secret-123456789"
[monitor]
metrics_bearer_token = "PRIVATE-SENTINEL-production-metrics-token-123456789"
[api_docs]
enabled = false
[database.primary]
host = "mysql.example.invalid"
tls_mode = "verify_identity"
tls_ca = "/example/mysql-ca.pem"
[redis]
mode = "required"
host = "redis.example.invalid"
tls = true
tls_ca = "/example/redis-ca.pem"
[cors]
allow_origins = ["https://admin.example.invalid"]
[object_storage]
backend = "s3"
endpoint = "https://storage.example.invalid"
access_key = "PRIVATE-SENTINEL-access"
secret_key = "PRIVATE-SENTINEL-storage"
use_ssl = true
[jobs]
mode = "external"
"#,
        )
        .unwrap();
        merge(&mut table, overlay);
    }
    table
}

fn merge(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(overlay)) => merge(base, overlay),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[test]
fn complete_selected_file_loads_for_each_environment_without_common_file() {
    for environment in [Environment::Dev, Environment::Test, Environment::Prod] {
        let fixture = Fixture::new();
        fixture.selected(
            environment,
            &toml::to_string(&complete(environment)).unwrap(),
        );
        let config = fixture.load(environment).unwrap();
        assert_eq!(config.environment, environment);
        assert!(fs::symlink_metadata(fixture.0.join("app.toml")).is_err());
        if environment == Environment::Prod {
            assert_eq!(config.jobs.mode, JobWorkerMode::External);
            assert!(!config.api_docs.enabled);
            assert!(config.auth.jwt_secret.len() >= 32);
            assert!(config.monitor.metrics_bearer_token.len() >= 32);
        }
    }
}

#[test]
fn selected_file_remains_required_when_common_is_absent() {
    for environment in [Environment::Dev, Environment::Test, Environment::Prod] {
        let fixture = Fixture::new();
        let error = fixture.error(environment);
        assert!(
            error.contains(&format!("app.{environment}.toml")),
            "{error}"
        );
        assert!(error.contains("无法读取"));
    }
}

#[test]
fn missing_required_field_in_standalone_environment_is_rejected() {
    let fixture = Fixture::new();
    let mut table = complete(Environment::Test);
    table.remove("app");
    fixture.selected(Environment::Test, &toml::to_string(&table).unwrap());
    let error = fixture.error(Environment::Test);
    assert!(error.contains("app.test.toml"));
    assert!(error.contains("app"));
    assert!(error.contains("字段反序列化失败"));
}

#[test]
fn existing_malformed_non_utf8_or_directory_common_is_not_ignored() {
    for invalid in [
        b"[auth]\njwt_secret = 'PRIVATE-SENTINEL".as_slice(),
        &[0xff, 0xfe],
    ] {
        let fixture = Fixture::new();
        fixture.selected(
            Environment::Test,
            &toml::to_string(&complete(Environment::Test)).unwrap(),
        );
        fs::write(fixture.0.join("app.toml"), invalid).unwrap();
        assert!(fixture.error(Environment::Test).contains("app.toml"));
    }
    let fixture = Fixture::new();
    fixture.selected(
        Environment::Test,
        &toml::to_string(&complete(Environment::Test)).unwrap(),
    );
    fs::create_dir(fixture.0.join("app.toml")).unwrap();
    fs::write(fixture.0.join("app.toml/PRIVATE-SENTINEL"), "private").unwrap();
    assert!(fixture.error(Environment::Test).contains("无法读取"));
}

#[test]
fn existing_common_type_error_is_not_ignored_or_exposed() {
    let fixture = Fixture::new();
    let mut table = complete(Environment::Test);
    table.insert(
        "auth".into(),
        toml::Value::String("PRIVATE-SENTINEL".into()),
    );
    fs::write(fixture.0.join("app.toml"), toml::to_string(&table).unwrap()).unwrap();
    fixture.selected(Environment::Test, "snowflake_worker_id = 7");
    let error = fixture.error(Environment::Test);
    assert!(error.contains("auth"));
    assert!(error.contains("字段反序列化失败"));
}

#[test]
fn standalone_production_retains_security_and_runtime_constraints() {
    for (section, field, value, expected) in [
        ("auth", "jwt_secret", "weak", "auth.jwt_secret"),
        (
            "monitor",
            "metrics_bearer_token",
            "weak",
            "metrics_bearer_token",
        ),
        ("jobs", "mode", "embedded", "jobs.mode"),
    ] {
        let fixture = Fixture::new();
        let mut table = complete(Environment::Prod);
        table[section]
            .as_table_mut()
            .unwrap()
            .insert(field.into(), value.into());
        fixture.selected(Environment::Prod, &toml::to_string(&table).unwrap());
        assert!(fixture.error(Environment::Prod).contains(expected));
    }
    for (section, field) in [("api_docs", "enabled"), ("redis", "tls")] {
        let fixture = Fixture::new();
        let mut table = complete(Environment::Prod);
        table[section]
            .as_table_mut()
            .unwrap()
            .insert(field.into(), (section == "api_docs").into());
        fixture.selected(Environment::Prod, &toml::to_string(&table).unwrap());
        assert!(fixture.error(Environment::Prod).contains(section));
    }
    for invalid_scope in [true, false] {
        let fixture = Fixture::new();
        let mut table = complete(Environment::Prod);
        let expected = if invalid_scope {
            table.insert("scope_id".into(), "replace-with-unique-scope".into());
            "scope_id"
        } else {
            table["database"]["primary"]
                .as_table_mut()
                .unwrap()
                .insert("tls_mode".into(), "disabled".into());
            "tls_mode"
        };
        fixture.selected(Environment::Prod, &toml::to_string(&table).unwrap());
        assert!(fixture.error(Environment::Prod).contains(expected));
    }
}

#[test]
fn dangling_common_link_is_an_error_and_is_not_replaced() {
    let fixture = Fixture::new();
    fixture.selected(
        Environment::Test,
        &toml::to_string(&complete(Environment::Test)).unwrap(),
    );
    let missing = fixture.0.join("missing-common-target.toml");
    let common = fixture.0.join("app.toml");
    create_link(&missing, &common);
    let target_before = fs::read_link(&common).unwrap();
    assert_eq!(
        fs::symlink_metadata(&target_before).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    let error = fixture.error(Environment::Test);
    assert!(error.contains("app.toml"));
    assert!(error.contains("无法读取"));
    assert_eq!(fs::read_link(&common).unwrap(), target_before);
}

#[cfg(unix)]
fn create_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[cfg(windows)]
fn create_link(target: &Path, link: &Path) {
    std::os::windows::fs::symlink_file(target, link)
        .expect("悬空链接回归测试需要 Windows 开发者模式或创建符号链接权限");
}
