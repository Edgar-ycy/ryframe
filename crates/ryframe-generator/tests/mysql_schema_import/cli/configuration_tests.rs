use super::*;
use ryframe_config::{AppConfig, DbConnection, Environment};

const SECRET: &str = "offline-shared-password-do-not-echo";
const BASE: &str = r#"snowflake_worker_id = 1
scope_id = "mysql-cli-shared"
[app]
name = "mysql-cli-offline"
host = "127.0.0.1"
port = 8080
[database.primary]
host = "localhost"
port = 13306
database = "original_schema"
username = "shared_user"
password = "offline-shared-password-do-not-echo"
max_connections = 7
min_connections = 1
acquire_timeout_secs = 11
idle_timeout_secs = 12
max_lifetime_secs = 13
connect_timeout_secs = 14
tls_mode = "verify_identity"
tls_ca = "shared-ca.pem"
tls_client_cert = "shared-cert.pem"
tls_client_key = "shared-key.pem"
[auth]
jwt_secret = "mysql-cli-offline-jwt-secret-long-enough"
access_token_expire = "1h"
refresh_token_expire = "168h"
[logger]
level = "info"
format = "text"
output = "stdout"
retention_days = 7
[jobs]
mode = "external"
health_port = 9091
[object_storage]
backend = "local"
local_base_dir = "must-not-create-storage"
"#;
const SELECTED: &str = "# 保留选中配置的完整捕获字节\nscope_id = 'mysql-cli-selected'\n[database.primary]\nport = 23306\n";

fn fixture() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|root| root.join("architecture/crate-boundaries.toml").is_file())
        .unwrap()
        .join(".local-tests/mysql-cli-configuration-offline");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn write_source(parent: &Path, name: &str, base: &str) -> PathBuf {
    let directory = parent.join(name);
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("app.toml"), base).unwrap();
    fs::write(directory.join("app.test.toml"), SELECTED).unwrap();
    directory
}

fn equal_connection_except_schema(left: &DbConnection, right: &DbConnection) -> bool {
    left.host == right.host
        && left.port == right.port
        && left.username == right.username
        && left.password == right.password
        && left.max_connections == right.max_connections
        && left.min_connections == right.min_connections
        && left.acquire_timeout_secs == right.acquire_timeout_secs
        && left.idle_timeout_secs == right.idle_timeout_secs
        && left.max_lifetime_secs == right.max_lifetime_secs
        && left.connect_timeout_secs == right.connect_timeout_secs
        && left.tls_mode == right.tls_mode
        && left.tls_ca == right.tls_ca
        && left.tls_client_cert == right.tls_client_cert
        && left.tls_client_key == right.tls_client_key
}

fn require_diagnostic(error: &str, path: &Path, stage: &str) {
    assert!(error.contains(stage));
    assert!(error.contains(path.to_string_lossy().as_ref()));
    assert!(
        error.contains("阶段=")
            && error.contains("路径=")
            && error.contains("原因=")
            && error.contains("处理=")
    );
    assert!(!error.contains(SECRET));
    assert!(!error.contains("password = '") && !error.contains("[broken"));
}

#[test]
fn explicit_shared_source_preserves_every_connection_field_and_only_changes_schema() {
    let scope = fixture();
    for with_base in [true, false] {
        let case = scope.path().join(if with_base {
            "with-base"
        } else {
            "selected-only"
        });
        fs::create_dir(&case).unwrap();
        let root = case.join("workspace");
        fs::create_dir(&root).unwrap();
        let public = write_source(
            &root,
            "config",
            &BASE
                .replace("shared", "public")
                .replace("localhost", "127.0.0.2"),
        );
        fs::write(
            public.join("app.test.toml"),
            SELECTED.replace("23306", "24306"),
        )
        .unwrap();
        let shared = write_source(&case, "explicit-shared", BASE);
        if !with_base {
            fs::remove_file(shared.join("app.toml")).unwrap();
            fs::write(shared.join("app.test.toml"), BASE).unwrap();
        }
        let original = (
            if with_base {
                Some(fs::read(shared.join("app.toml")).unwrap())
            } else {
                None
            },
            fs::read(shared.join("app.test.toml")).unwrap(),
        );
        let captured = CapturedConfiguration::capture(&shared).unwrap();
        assert!(captured.base.as_ref().map(|file| &file.bytes) == original.0.as_ref());
        assert!(captured.selected.bytes == original.1);
        let prepared = captured.prepare(&root, "actual_fixture_schema").unwrap();
        let loaded = AppConfig::load(prepared.directory().unwrap(), Environment::Test).unwrap();
        let expected = AppConfig::load(&shared, Environment::Test).unwrap();
        let unrelated = AppConfig::load(&public, Environment::Test).unwrap();
        assert!(equal_connection_except_schema(
            &loaded.database.primary,
            &expected.database.primary
        ));
        assert!(!equal_connection_except_schema(
            &loaded.database.primary,
            &unrelated.database.primary
        ));
        assert!(loaded.database.primary.database == "actual_fixture_schema");
        let mut expected_table: toml::Table =
            toml::from_str(std::str::from_utf8(&original.1).unwrap()).unwrap();
        expected_table["database"]["primary"]
            .as_table_mut()
            .unwrap()
            .insert(
                "database".into(),
                toml::Value::String("actual_fixture_schema".into()),
            );
        assert!(
            parse_table(
                &prepared.selected,
                &prepared.directory.join("app.test.toml")
            )
            .unwrap()
                == expected_table
        );
        assert!(fs::read(prepared.directory.join("app.toml")).ok() == original.0);
        assert_eq!(prepared.directory.join("app.toml").exists(), with_base);
        assert!(fs::read(shared.join("app.toml")).ok() == original.0);
        assert!(fs::read(shared.join("app.test.toml")).unwrap() == original.1);
        assert!(
            fs::read(public.join("app.toml")).unwrap()
                == BASE
                    .replace("shared", "public")
                    .replace("localhost", "127.0.0.2")
                    .as_bytes()
        );
    }
}

#[test]
fn captured_source_changes_are_rejected_without_recapturing_or_creating_target() {
    let scope = fixture();
    for (index, name, absent, replacement) in [
        (0, "app.toml", false, false),
        (1, "app.test.toml", false, false),
        (2, "app.toml", true, false),
        (3, "app.toml", false, true),
    ] {
        let case = scope.path().join(format!("case-{index}"));
        fs::create_dir(&case).unwrap();
        let shared = write_source(&case, "source", BASE);
        if absent {
            fs::remove_file(shared.join(name)).unwrap();
        }
        let captured = CapturedConfiguration::capture(&shared).unwrap();
        let root = case.join("workspace");
        fs::create_dir(&root).unwrap();
        if replacement {
            fs::rename(shared.join(name), shared.join("retained-original.toml")).unwrap();
            fs::write(shared.join(name), BASE).unwrap();
            File::options()
                .write(true)
                .open(shared.join(name))
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
                .unwrap();
        } else {
            fs::write(shared.join(name), format!("changed = '{SECRET}'")).unwrap();
        }
        let error = captured.prepare(&root, "fixture_schema").err().unwrap();
        require_diagnostic(&error, &shared.join(name), "复核共享源文件");
        assert!(!root.join(".local-tests/导入 配置").exists());
    }
}

#[test]
fn source_drift_after_preparation_is_rejected_and_private_snapshot_stays_unchanged() {
    let scope = fixture();
    for (index, private, absent) in [(0, false, false), (1, false, true), (2, true, true)] {
        let case = scope.path().join(format!("case-{index}"));
        fs::create_dir(&case).unwrap();
        let shared = write_source(&case, "source", BASE);
        if absent {
            fs::remove_file(shared.join("app.toml")).unwrap();
            fs::write(shared.join("app.test.toml"), BASE).unwrap();
        }
        let root = case.join("workspace");
        fs::create_dir(&root).unwrap();
        let prepared = CapturedConfiguration::capture(&shared)
            .unwrap()
            .prepare(&root, "fixture_schema")
            .unwrap();
        let before = fs::read(prepared.directory.join("app.test.toml")).unwrap();
        let changed = if private {
            prepared.directory.join("app.toml")
        } else {
            shared.join(if absent { "app.toml" } else { "app.test.toml" })
        };
        fs::write(&changed, "scope_id = 'changed-scope'").unwrap();
        let error = prepared.directory().err().unwrap();
        require_diagnostic(
            &error,
            &changed,
            if private {
                "复核私有配置文件"
            } else {
                "复核共享源文件"
            },
        );
        assert!(fs::read(prepared.directory.join("app.test.toml")).unwrap() == before);
        assert!(fs::read_to_string(changed).unwrap() == "scope_id = 'changed-scope'");
    }
}

#[test]
fn existing_target_and_later_foreign_file_are_never_overwritten() {
    let scope = fixture();
    let shared = write_source(scope.path(), "source", BASE);
    let root = scope.path().join("workspace");
    let destination = root.join(".local-tests").join("导入 配置");
    fs::create_dir_all(&destination).unwrap();
    let foreign = destination.join("app.toml");
    fs::write(&foreign, SECRET).unwrap();
    let error = CapturedConfiguration::capture(&shared)
        .unwrap()
        .prepare(&root, "fixture_schema")
        .err()
        .unwrap();
    require_diagnostic(&error, &destination, "准备私有配置/创建独占目录");
    assert!(error.contains("IOkind=AlreadyExists") && error.contains("rawcode="));
    assert!(error.contains("不自动重放"));
    assert!(fs::read(&foreign).unwrap() == SECRET.as_bytes());
    let publish_error = publish_new(&foreign, b"replacement").unwrap_err();
    require_diagnostic(&publish_error, &foreign, "发布私有配置/创建文件");
    assert!(publish_error.contains("目标目录已创建") && publish_error.contains("部分文件保留"));
    assert!(publish_error.contains("IOkind=AlreadyExists") && publish_error.contains("rawcode="));
    assert!(fs::read(&foreign).unwrap() == SECRET.as_bytes());
    fs::remove_file(&foreign).unwrap();
    fs::remove_dir(&destination).unwrap();
    let prepared = CapturedConfiguration::capture(&shared)
        .unwrap()
        .prepare(&root, "fixture_schema")
        .unwrap();
    fs::write(prepared.directory.join("app.toml"), SECRET).unwrap();
    let error = prepared.directory().err().unwrap();
    require_diagnostic(
        &error,
        &prepared.directory.join("app.toml"),
        "复核私有配置文件",
    );
    assert!(error.contains("不重新捕获、不切换来源、不自动重放"));
    assert!(fs::read(prepared.directory.join("app.toml")).unwrap() == SECRET.as_bytes());
}

#[test]
fn missing_non_regular_invalid_and_non_utf8_sources_fail_with_safe_diagnostics() {
    let scope = fixture();
    let source = scope.path().join("source");
    fs::create_dir(&source).unwrap();
    let error = CapturedConfiguration::capture(&source).err().unwrap();
    require_diagnostic(&error, &source.join("app.test.toml"), "读取前元数据");
    assert!(error.contains("IOkind=NotFound") && error.contains("rawcode="));
    fs::write(source.join("app.toml"), BASE).unwrap();
    fs::create_dir(source.join("app.test.toml")).unwrap();
    let error = CapturedConfiguration::capture(&source).err().unwrap();
    require_diagnostic(
        &error,
        &source.join("app.test.toml"),
        "捕获共享源/app.test.toml",
    );
    assert!(error.contains("非普通文件"));
    fs::remove_dir(source.join("app.test.toml")).unwrap();
    for (bytes, stage, location) in [
        (
            format!("password = '{SECRET}'\n[broken").into_bytes(),
            "解析共享配置/TOML",
            "byte/span=",
        ),
        (vec![0xff, 0xfe], "解析共享配置/UTF8", "byte=0"),
    ] {
        fs::write(source.join("app.test.toml"), bytes).unwrap();
        let error = CapturedConfiguration::capture(&source).err().unwrap();
        require_diagnostic(&error, &source.join("app.test.toml"), stage);
        assert!(error.contains(location));
        if stage.ends_with("TOML") {
            assert!(error.contains("行=2") && error.contains("列="));
        }
    }
    for (index, text, field) in [
        (0, format!("database = '{SECRET}'"), "database"),
        (
            1,
            format!("[database]\nprimary = '{SECRET}'"),
            "database.primary",
        ),
    ] {
        fs::write(source.join("app.test.toml"), text).unwrap();
        let root = scope.path().join(format!("invalid-field-{index}"));
        fs::create_dir(&root).unwrap();
        let error = CapturedConfiguration::capture(&source)
            .unwrap()
            .prepare(&root, "fixture_schema")
            .err()
            .unwrap();
        require_diagnostic(
            &error,
            &source.join("app.test.toml"),
            "准备私有配置/修改schema",
        );
        assert!(error.contains(&format!("字段 {field} 必须是 TOML 表")));
        assert!(
            error.contains("目标目录已创建")
                && error.contains("部分文件保留")
                && error.contains("不自动重放")
        );
        assert!(root.join(".local-tests/导入 配置").is_dir());
    }
    fs::write(source.join("app.test.toml"), SELECTED).unwrap();
    let unknown = source.join("unknown.toml");
    fs::write(&unknown, format!("password = '{SECRET}'\n[broken")).unwrap();
    assert!(CapturedConfiguration::capture(&source).is_ok());
    assert!(fs::read_to_string(&unknown).unwrap().contains(SECRET));
    let custom_io = io::Error::new(io::ErrorKind::PermissionDenied, SECRET);
    let error = io_error("离线IO诊断", &source, &custom_io, "检查读取权限");
    require_diagnostic(&error, &source, "离线IO诊断");
    assert!(error.contains("IOkind=PermissionDenied") && error.contains("rawcode=None"));
    let error = io_error(
        "离线系统错误码",
        &source,
        &io::Error::from_raw_os_error(5),
        "核对系统错误码",
    );
    require_diagnostic(&error, &source, "离线系统错误码");
    assert!(error.contains("rawcode=Some(5)"));
}

#[cfg(any(unix, windows))]
#[test]
fn linked_source_file_and_linked_ancestor_are_rejected() {
    let scope = fixture();
    let source = write_source(scope.path(), "source", BASE);
    let link = scope.path().join("link");
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&source, &link);
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&source, &link);
    if let Err(error) = linked {
        #[cfg(windows)]
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            eprintln!("链接场景未验收：当前账户无创建符号链接权限");
            return;
        }
        panic!("创建离线测试链接失败：{error}");
    }
    assert!(CapturedConfiguration::capture(&link).is_err());
    let base = source.join("app.toml");
    fs::remove_file(&base).unwrap();
    let captured = CapturedConfiguration::capture(&source).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(source.join("app.test.toml"), &base).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(source.join("app.test.toml"), &base).unwrap();
    assert!(captured.verify().is_err());
    assert!(CapturedConfiguration::capture(&source).is_err());
    fs::remove_file(&base).unwrap();
    fs::write(&base, BASE).unwrap();
    let selected = source.join("app.test.toml");
    fs::remove_file(&selected).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(source.join("app.toml"), &selected).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(source.join("app.toml"), &selected).unwrap();
    assert!(CapturedConfiguration::capture(&source).is_err());
}
