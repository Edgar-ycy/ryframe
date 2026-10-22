use super::*;

static DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const SECRET: &str = "offline-mysql-secret-do-not-print";
const BASE: &str = r#"snowflake_worker_id = 1
scope_id = "mysql-file-fixture"
[app]
name = "mysql-file-fixture"
host = "127.0.0.1"
port = 8080
[database.primary]
host = "localhost"
port = 13306
database = "must_not_connect"
username = "offline_user"
password = "offline-mysql-secret-do-not-print"
max_connections = 7
min_connections = 1
acquire_timeout_secs = 11
idle_timeout_secs = 12
max_lifetime_secs = 13
connect_timeout_secs = 14
tls_mode = "verify_identity"
tls_ca = "fixture-ca.pem"
tls_client_cert = "fixture-cert.pem"
tls_client_key = "fixture-key.pem"
[auth]
jwt_secret = "offline-file-configuration-fixture-secret"
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

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let root = workspace_root(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let parent = root.join(".local-tests/mysql-helper-offline");
        fs::create_dir_all(&parent).unwrap();
        assert!(integration_enabled_at(&parent).unwrap());
        loop {
            let sequence = DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{}-{sequence}", process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("创建离线配置测试目录失败：{error}"),
            }
        }
    }

    fn config(&self) -> PathBuf {
        self.0.join("config")
    }

    fn write(&self) -> PathBuf {
        let config = self.config();
        fs::create_dir(&config).unwrap();
        fs::write(config.join("app.toml"), BASE).unwrap();
        fs::write(config.join("app.test.toml"), "").unwrap();
        config
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        // 仅清理本次 create_dir 独占创建的离线目录，不接触共享配置目录。
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn rejected(directory: &Path) -> String {
    match load_configuration_at(directory) {
        Ok(_) => panic!("错误配置不得被接受"),
        Err(error) => {
            assert!(!error.contains(SECRET));
            assert!(!error.contains("offline-file-configuration-fixture-secret"));
            error
        }
    }
}

#[test]
fn explicit_files_supply_scope_connection_tls_and_pool_without_connecting() {
    let fixture = Directory::new();
    let directory = fixture.write();
    assert!(integration_enabled_at(&directory).unwrap());
    let config = load_configuration_at(&directory).unwrap();
    assert_eq!(config.scope_id.as_str(), "mysql-file-fixture");
    let primary = &config.database.primary;
    assert_eq!(primary.host, "localhost");
    assert_eq!(primary.port, 13306);
    assert_eq!(primary.database, "must_not_connect");
    assert_eq!(primary.username, "offline_user");
    assert_eq!(primary.password, SECRET);
    assert_eq!((primary.max_connections, primary.min_connections), (7, 1));
    assert_eq!(primary.acquire_timeout_secs, 11);
    assert_eq!(primary.idle_timeout_secs, 12);
    assert_eq!(primary.max_lifetime_secs, 13);
    assert_eq!(primary.connect_timeout_secs, 14);
    assert_eq!(primary.tls_mode, DbTlsMode::VerifyIdentity);
    assert_eq!(primary.tls_ca.as_deref(), Some("fixture-ca.pem"));
    assert_eq!(primary.tls_client_cert.as_deref(), Some("fixture-cert.pem"));
    assert_eq!(primary.tls_client_key.as_deref(), Some("fixture-key.pem"));
}

#[test]
fn connection_clones_preserve_snapshot_and_only_explicit_tls_case_changes_mode() {
    let fixture = Directory::new();
    let directory = fixture.write();
    let config = load_configuration_at(&directory).unwrap();
    fs::write(directory.join("app.toml"), "invalid TOML").unwrap();
    let primary = &config.database.primary;
    for (database, mode) in [
        ("mysql", None),
        ("isolated_schema", None),
        ("mysql", Some(DbTlsMode::Required)),
        ("mysql", Some(DbTlsMode::VerifyCa)),
    ] {
        let cloned = connection_config(primary, database, mode).unwrap();
        assert_eq!(cloned.database, database);
        assert_eq!(cloned.tls_mode, mode.unwrap_or(primary.tls_mode));
        assert_eq!(cloned.password, primary.password);
        assert_eq!(cloned.host, primary.host);
        assert_eq!(cloned.port, primary.port);
        assert_eq!(cloned.username, primary.username);
        assert_eq!(cloned.max_connections, primary.max_connections);
        assert_eq!(cloned.min_connections, primary.min_connections);
        assert_eq!(cloned.acquire_timeout_secs, primary.acquire_timeout_secs);
        assert_eq!(cloned.idle_timeout_secs, primary.idle_timeout_secs);
        assert_eq!(cloned.max_lifetime_secs, primary.max_lifetime_secs);
        assert_eq!(cloned.connect_timeout_secs, primary.connect_timeout_secs);
        assert_eq!(cloned.tls_ca, primary.tls_ca);
        assert_eq!(cloned.tls_client_cert, primary.tls_client_cert);
        assert_eq!(cloned.tls_client_key, primary.tls_client_key);
    }
    assert!(connection_config(primary, "mysql", Some(DbTlsMode::Disabled)).is_err());
}

#[test]
fn absent_directory_is_the_only_opt_out_and_missing_files_are_errors() {
    let fixture = Directory::new();
    let directory = fixture.config();
    assert!(!integration_enabled_at(&directory).unwrap());
    assert!(rejected(&directory).contains("未验收"));
    fs::create_dir(&directory).unwrap();
    assert!(integration_enabled_at(&directory).unwrap());
    assert!(rejected(&directory).contains("app.test.toml"));
    fs::write(directory.join("app.toml"), BASE).unwrap();
    assert!(rejected(&directory).contains("app.test.toml"));
}

#[test]
fn selected_only_configuration_uses_the_same_validated_snapshot_without_writes() {
    let fixture = Directory::new();
    let directory = fixture.write();
    fs::write(directory.join("app.test.toml"), BASE).unwrap();
    fs::remove_file(directory.join("app.toml")).unwrap();
    let before = fs::read(directory.join("app.test.toml")).unwrap();
    let config = load_configuration_at(&directory).unwrap();
    assert_eq!(config.scope_id.as_str(), "mysql-file-fixture");
    assert_eq!(config.database.primary.password, SECRET);
    assert_eq!(config.database.primary.tls_mode, DbTlsMode::VerifyIdentity);
    assert_eq!(fs::read(directory.join("app.test.toml")).unwrap(), before);
    assert!(!directory.join("app.toml").exists());
    fs::write(directory.join("app.toml"), "[broken").unwrap();
    assert!(rejected(&directory).contains("TOML 语法错误"));
}

#[test]
fn non_directory_opt_in_and_non_file_configuration_are_rejected() {
    let fixture = Directory::new();
    fs::write(fixture.config(), "not a directory").unwrap();
    assert!(integration_enabled_at(&fixture.config()).is_err());
    assert!(integration_enabled_at(&fixture.config().join("missing")).is_err());
    fs::remove_file(fixture.config()).unwrap();
    let directory = fixture.write();
    for name in ["app.toml", "app.test.toml"] {
        let file = directory.join(name);
        let bytes = fs::read(&file).unwrap();
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        assert!(rejected(&directory).contains("普通无链接文件"));
        fs::remove_dir(&file).unwrap();
        fs::write(file, bytes).unwrap();
    }
}

#[test]
fn invalid_toml_types_fields_and_disabled_tls_fail_without_echoing_secrets() {
    let fixture = Directory::new();
    let directory = fixture.write();
    for invalid in [
        format!("password = \"{SECRET}\"\n[broken"),
        BASE.replace("port = 13306", &format!("port = \"{SECRET}\"")),
        BASE.replace("port = 13306", "port = 0"),
        BASE.replace("tls_mode = \"verify_identity\"", "tls_mode = \"disabled\""),
        BASE.replace("tls_ca = \"fixture-ca.pem\"", "tls_ca = \"\""),
        BASE.replace(
            "tls_client_key = \"fixture-key.pem\"",
            "tls_client_key = \"\"",
        ),
    ] {
        fs::write(directory.join("app.toml"), invalid).unwrap();
        assert!(!rejected(&directory).is_empty());
    }
    fs::write(directory.join("app.toml"), BASE).unwrap();
    fs::write(
        directory.join("app.test.toml"),
        format!("scope_id = \"{SECRET}\"\n[broken"),
    )
    .unwrap();
    assert!(rejected(&directory).contains("app.test.toml"));
}

#[test]
fn environment_file_overrides_base_and_required_tls_remains_valid() {
    let fixture = Directory::new();
    let directory = fixture.write();
    fs::write(
        directory.join("app.test.toml"),
        "scope_id = \"mysql-overlay\"\n[database.primary]\nport = 13307\ntls_mode = \"required\"\n",
    )
    .unwrap();
    let config = load_configuration_at(&directory).unwrap();
    assert_eq!(config.scope_id.as_str(), "mysql-overlay");
    assert_eq!(config.database.primary.port, 13307);
    assert_eq!(config.database.primary.tls_mode, DbTlsMode::Required);
    assert_eq!(config.database.primary.password, SECRET);
    fs::write(
        directory.join("app.test.toml"),
        "[database.primary]\ntls_mode = \"disabled\"\n",
    )
    .unwrap();
    assert!(rejected(&directory).contains("必须启用 TLS"));
}

#[test]
fn schemas_keep_safe_scope_test_pid_and_sequence_isolation() {
    let first = unique_schema("Mysql-Scope_1234567890", "TEST-invalid;_/1234567890");
    let second = unique_schema("Mysql-Scope_1234567890", "TEST-invalid;_/1234567890");
    assert_ne!(first, second);
    assert!(first.starts_with(&format!(
        "ryframe_it_mysqlscope123456_testinvalid12345_{}_",
        process::id()
    )));
    assert!(
        first
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
    assert!(first.len() <= 64);
}

#[cfg(any(unix, windows))]
#[test]
fn linked_files_directories_ancestors_and_dangling_links_are_not_opt_outs() {
    let fixture = Directory::new();
    let directory = fixture.write();
    let link = fixture.0.join("link");
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(&directory, &link);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(&directory, &link);
    if let Err(error) = result {
        #[cfg(windows)]
        if error.kind() == io::ErrorKind::PermissionDenied {
            eprintln!("链接场景未验收：当前 Windows 账户无创建符号链接权限");
            return;
        }
        panic!("创建离线目录链接失败：{error}");
    }
    assert!(integration_enabled_at(&link).is_err());
    assert!(integration_enabled_at(&link.join("missing")).is_err());
    assert!(load_configuration_at(&link).is_err());
    #[cfg(unix)]
    fs::remove_file(&link).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&link).unwrap();
    for name in ["app.toml", "app.test.toml"] {
        let file = directory.join(name);
        let bytes = fs::read(&file).unwrap();
        fs::remove_file(&file).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            directory.join(if name == "app.toml" {
                "app.test.toml"
            } else {
                "app.toml"
            }),
            &file,
        )
        .unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(
            directory.join(if name == "app.toml" {
                "app.test.toml"
            } else {
                "app.toml"
            }),
            &file,
        )
        .unwrap();
        assert!(rejected(&directory).contains("普通无链接文件"));
        fs::remove_file(&file).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(fixture.0.join("absent"), &file).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(fixture.0.join("absent"), &file).unwrap();
        assert!(rejected(&directory).contains("普通无链接文件"));
        fs::remove_file(&file).unwrap();
        fs::write(&file, bytes).unwrap();
    }
    fs::remove_dir_all(&directory).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(fixture.0.join("absent"), &directory).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(fixture.0.join("absent"), &directory).unwrap();
    assert!(integration_enabled_at(&directory).is_err());
}
