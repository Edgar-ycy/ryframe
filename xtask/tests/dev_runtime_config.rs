use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use ryframe_config::{AppConfig, Environment, JobWorkerMode, LoggerOutput, MigrationMode};
use tempfile::TempDir;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[path = "../src/dev/runtime_config.rs"]
mod runtime_config;

use runtime_config::{RuntimeConfig, RuntimeProfile};

struct Fixture {
    directory: TempDir,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests/dev-runtime-config");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let source = directory.path().join("source");
        fs::create_dir(&source).unwrap();
        write_table(&source.join("app.toml"), &base_table());
        fs::write(source.join("app.dev.toml"),
            "# 原有角色覆盖应保留\n[app]\nname = '角色 测试'\n[database.primary]\npassword = 'overlay-db-private'\n[jobs]\nworker_id = 'original-worker'\n[telemetry]\nenabled = true\n[logger]\noutput = 'file'\n").unwrap();
        fs::write(
            source.join("app.test.toml"),
            "# 测试环境\n[app]\nport = 18081\n",
        )
        .unwrap();
        fs::write(
            source.join("app.prod.toml"),
            "# 原样保留生产文件\n[auth]\njwt_secret = 'prod-private-value'\n",
        )
        .unwrap();
        fs::write(
            source.join("tools.toml"),
            "[generator]\nconfig_dir = '本地 配置'\n",
        )
        .unwrap();
        fs::write(
            source.join("feature-matrix.json"),
            "{\"version\":1,\"packages\":[\"first\",\"second\"]}\n",
        )
        .unwrap();
        Self { directory, source }
    }

    fn target(&self, name: &str) -> PathBuf {
        self.directory.path().join(name)
    }

    fn change(&self, name: &str, edit: impl FnOnce(&mut toml::Table)) {
        let path = self.source.join(name);
        let mut table = table_at(&path);
        edit(&mut table);
        write_table(&path, &table);
    }
}

fn base_table() -> toml::Table {
    let mut table: toml::Table = toml::from_str(include_str!("../../config/app.toml")).unwrap();
    table["bootstrap"]
        .as_table_mut()
        .unwrap()
        .insert("admin_password".into(), "bootstrap-private".into());
    table["database"]["primary"]["password"] = "base-db-private".into();
    table["auth"]["jwt_secret"] = "jwt-private".into();
    table["redis"]["password"] = "redis-private".into();
    table["object_storage"]["access_key"] = "access-private".into();
    table["object_storage"]["secret_key"] = "storage-private".into();
    table["monitor"]["metrics_bearer_token"] = "metrics-private".into();
    let primary = table["database"]["primary"].clone();
    let connection = |name: &str, password: &str| {
        let mut value = primary.clone();
        value
            .as_table_mut()
            .unwrap()
            .insert("name".into(), name.into());
        value["database"] = name.into();
        value["password"] = password.into();
        value
    };
    table["database"].as_table_mut().unwrap().insert(
        "replicas".into(),
        vec![
            connection("replica_one", "replica-one-private"),
            connection("replica_two", "replica-two-private"),
        ]
        .into(),
    );
    table["database"].as_table_mut().unwrap().insert(
        "sources".into(),
        vec![
            connection("source_one", "source-one-private"),
            connection("source_two", "source-two-private"),
        ]
        .into(),
    );
    table["tenant_data"].as_table_mut().unwrap().insert("targets".into(), toml::from_str::<toml::Table>(
        "[[targets]]\nkey = 'tenant-one'\nmode = 'dedicated'\nkind = 'mysql'\nhost = 'localhost'\ndatabase = 'tenant_one'\nusername = 'tenant'\npassword = 'tenant-private'\n\n[[targets]]\nkey = 'tenant-two'\nmode = 'dedicated'\nkind = 'mysql'\nhost = 'localhost'\ndatabase = 'tenant_two'\nusername = 'tenant'\npassword = ''\n",
    ).unwrap().remove("targets").unwrap());
    table["cors"]["allow_origins"] = vec![
        toml::Value::String("http://localhost:5173".into()),
        toml::Value::String("http://localhost:5174".into()),
    ]
    .into();
    table
}

fn table_at(path: &Path) -> toml::Table {
    toml::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn write_table(path: &Path, table: &toml::Table) {
    fs::write(path, toml::to_string(table).unwrap()).unwrap();
}

fn set(table: &mut toml::Table, group: &str, key: &str, value: impl Into<toml::Value>) {
    table[group]
        .as_table_mut()
        .unwrap()
        .insert(key.into(), value.into());
}

fn files_at(path: &Path) -> BTreeMap<String, Vec<u8>> {
    fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn normal(worker: bool) -> RuntimeProfile {
    RuntimeProfile {
        api_port: 18080,
        worker_port: 19091,
        snowflake_worker_id: 1023,
        probe: false,
        worker,
    }
}

fn assert_error_redacted(error: impl std::fmt::Display) {
    let message = error.to_string();
    for secret in ["private", "password =", "jwt_secret =", "must-not-echo"] {
        assert!(!message.contains(secret), "错误不应包含配置值或原始行");
    }
}

#[test]
fn snapshot_preserves_every_file_byte_all_plaintext_credentials_and_arrays() {
    let fixture = Fixture::new();
    let before = files_at(&fixture.source);
    let source = fixture.source.canonicalize().unwrap();
    let config = RuntimeConfig::capture(&source).unwrap();
    let target = fixture
        .directory
        .path()
        .canonicalize()
        .unwrap()
        .join("snapshot");
    config.snapshot_to(&target).unwrap();
    assert_eq!(files_at(&target), before);
    assert_eq!(files_at(&fixture.source), before);
    assert_eq!(config.app().database.primary.password, "overlay-db-private");
    assert_eq!(
        config.app().bootstrap.admin_password.as_deref(),
        Some("bootstrap-private")
    );
    assert_eq!(config.app().database.replicas.len(), 2);
    assert_eq!(config.app().database.sources.len(), 2);
    assert_eq!(config.app().tenant_data.targets.len(), 2);
    assert_eq!(
        config.app().tenant_data.targets[1].password.as_deref(),
        Some("")
    );
    let recovered = RuntimeConfig::capture(&target).unwrap();
    assert!(config.semantically_matches(&recovered).unwrap());
}

#[test]
fn complete_dev_file_does_not_require_optional_common_defaults() {
    let fixture = Fixture::new();
    write_table(&fixture.source.join("app.dev.toml"), &base_table());
    fs::remove_file(fixture.source.join("app.toml")).unwrap();
    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    let snapshot = fixture.target("snapshot");
    config.snapshot_to(&snapshot).unwrap();
    assert!(!snapshot.join("app.toml").exists());
    assert_eq!(files_at(&snapshot), files_at(&fixture.source));
    let role = fixture.target("role");
    config
        .write_profile(&role, normal(true), &fixture.target("locales"))
        .unwrap();
    assert!(!role.join("app.toml").exists());
    assert_eq!(
        AppConfig::load(&role, Environment::Dev).unwrap().app.port,
        18080
    );
}

#[test]
fn debug_contains_only_registered_file_names_and_count() {
    let fixture = Fixture::new();
    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    let debug = format!("{config:?}");
    assert!(debug.contains("file_count: 6"));
    assert!(debug.contains("app.dev.toml"));
    assert!(!debug.contains("private"));
    assert!(!debug.contains("角色"));
    assert!(!debug.contains("18080"));
}

#[test]
fn snapshots_use_captured_values_after_source_changes_or_disappears() {
    let fixture = Fixture::new();
    let before = files_at(&fixture.source);
    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    fixture.change("app.dev.toml", |table| {
        table["database"]["primary"]["password"] = "changed-private".into()
    });
    fs::remove_dir_all(&fixture.source).unwrap();
    let target = fixture.target("snapshot");
    config.snapshot_to(&target).unwrap();
    assert_eq!(files_at(&target), before);
    config
        .write_profile(
            &fixture.target("role"),
            normal(true),
            &fixture.target("最终 locales"),
        )
        .unwrap();
    let app = AppConfig::load(fixture.target("role"), Environment::Dev).unwrap();
    assert_eq!(app.database.primary.password, "overlay-db-private");
}

#[test]
fn comments_formatting_and_json_object_key_order_are_semantically_equal() {
    let fixture = Fixture::new();
    let before = RuntimeConfig::capture(&fixture.source).unwrap();
    let path = fixture.source.join("app.dev.toml");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(path, format!("# 仅排版\n\n{original}\n# 保存\n")).unwrap();
    fs::write(
        fixture.source.join("feature-matrix.json"),
        "{ \"packages\": [\"first\", \"second\"], \"version\": 1 }",
    )
    .unwrap();
    fixture.change("tools.toml", |_| {});
    let after = RuntimeConfig::capture(&fixture.source).unwrap();
    assert!(before.semantically_matches(&after).unwrap());
}

#[test]
fn every_plaintext_credential_change_is_semantically_different() {
    let cases: &[&[&str]] = &[
        &["bootstrap", "admin_password"],
        &["auth", "jwt_secret"],
        &["redis", "password"],
        &["object_storage", "access_key"],
        &["object_storage", "secret_key"],
        &["monitor", "metrics_bearer_token"],
    ];
    for path in cases {
        let fixture = Fixture::new();
        let before = RuntimeConfig::capture(&fixture.source).unwrap();
        fixture.change("app.toml", |table| {
            table[path[0]][path[1]] = "changed-private".into()
        });
        assert!(
            !before
                .semantically_matches(&RuntimeConfig::capture(&fixture.source).unwrap())
                .unwrap()
        );
    }
    let fixture = Fixture::new();
    let before = RuntimeConfig::capture(&fixture.source).unwrap();
    fixture.change("app.dev.toml", |table| {
        table["database"]["primary"]["password"] = "changed-private".into()
    });
    assert!(
        !before
            .semantically_matches(&RuntimeConfig::capture(&fixture.source).unwrap())
            .unwrap()
    );
}

#[test]
fn array_order_values_credentials_and_missing_files_remain_significant() {
    for name in ["replicas", "sources"] {
        for reorder in [true, false] {
            let fixture = Fixture::new();
            let before = RuntimeConfig::capture(&fixture.source).unwrap();
            fixture.change("app.toml", |table| {
                let array = table["database"][name].as_array_mut().unwrap();
                if reorder {
                    array.swap(0, 1);
                } else {
                    array[0]["password"] = "changed-private".into();
                }
            });
            assert!(
                !before
                    .semantically_matches(&RuntimeConfig::capture(&fixture.source).unwrap())
                    .unwrap()
            );
        }
    }
    let fixture = Fixture::new();
    let before = RuntimeConfig::capture(&fixture.source).unwrap();
    fixture.change("app.toml", |table| {
        table["tenant_data"]["targets"].as_array_mut().unwrap()[0]["password"] =
            "changed-private".into()
    });
    assert!(
        !before
            .semantically_matches(&RuntimeConfig::capture(&fixture.source).unwrap())
            .unwrap()
    );
    fs::remove_file(fixture.source.join("tools.toml")).unwrap();
    assert!(
        !before
            .semantically_matches(&RuntimeConfig::capture(&fixture.source).unwrap())
            .unwrap()
    );
}

#[test]
fn json_array_order_and_non_development_file_changes_are_significant() {
    for (name, content) in [
        (
            "feature-matrix.json",
            "{\"version\":1,\"packages\":[\"second\",\"first\"]}",
        ),
        ("app.prod.toml", "[auth]\njwt_secret = 'changed-private'\n"),
        ("app.test.toml", "[app]\nport = 18082\n"),
        ("tools.toml", "[generator]\nconfig_dir = 'different'\n"),
    ] {
        let fixture = Fixture::new();
        let before = RuntimeConfig::capture(&fixture.source).unwrap();
        fs::write(fixture.source.join(name), content).unwrap();
        assert!(
            !before
                .semantically_matches(&RuntimeConfig::capture(&fixture.source).unwrap())
                .unwrap()
        );
    }
}

#[test]
fn parse_type_and_validation_errors_never_echo_secret_values_or_toml_lines() {
    for (name, content) in [
        ("app.dev.toml", "[auth]\njwt_secret = 'must-not-echo"),
        (
            "app.dev.toml",
            "[database.primary]\npassword = ['must-not-echo']\n",
        ),
        ("app.dev.toml", "[app]\nport = 'must-not-echo'\n"),
        ("app.dev.toml", "[app]\nport = 0\nname = 'must-not-echo'\n"),
        ("app.dev.toml", "[app]\nunknown = 'must-not-echo'\n"),
        (
            "app.dev.toml",
            "[database.primary]\npassword_env = 'must-not-echo'\n",
        ),
        ("tools.toml", "key = 'must-not-echo"),
        ("app.prod.toml", "key = 'must-not-echo"),
        ("feature-matrix.json", "{\"must-not-echo\":invalid}"),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.source.join(name), content).unwrap();
        assert_error_redacted(RuntimeConfig::capture(&fixture.source).unwrap_err());
    }
    let fixture = Fixture::new();
    fs::write(fixture.source.join("app.test.toml"), [0xff]).unwrap();
    assert_error_redacted(RuntimeConfig::capture(&fixture.source).unwrap_err());
}

#[test]
fn application_errors_retain_stage_source_field_reason_and_repair() {
    for (content, field, reason) in [
        ("[auth]\njwt_secret = ''\n", "auth.jwt_secret", "不能为空"),
        (
            "[auth]\naccess_token_expire = 'must-not-echo'\n",
            "auth.access_token_expire",
            "not a valid duration",
        ),
        (
            "[jobs]\npoll_interval_ms = 0\n",
            "jobs.poll_interval_ms",
            "必须在 50 到 60000 之间",
        ),
        (
            "[database.primary]\nport = 0\n",
            "database.primary.port",
            "必须大于 0",
        ),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.source.join("app.dev.toml"), content).unwrap();
        let message = RuntimeConfig::capture(&fixture.source)
            .unwrap_err()
            .to_string();
        assert!(message.contains("捕获开发配置"));
        assert!(message.contains(fixture.source.to_str().unwrap()));
        assert!(message.contains("app.dev.toml"));
        assert!(message.contains(field), "错误必须保留字段位置");
        assert!(message.contains(reason), "错误必须保留实际原因");
        assert!(message.contains("请修正"));
        assert_error_redacted(message);
    }
}

#[test]
fn parser_errors_keep_safe_positions_and_repairs_without_source_excerpts() {
    for (name, content, location) in [
        (
            "app.dev.toml",
            "[auth]\njwt_secret = 'must-not-echo",
            "字节位置",
        ),
        (
            "feature-matrix.json",
            "{\n\"must-not-echo\": invalid\n}",
            "line=2，column=",
        ),
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.source.join(name), content).unwrap();
        let message = RuntimeConfig::capture(&fixture.source)
            .unwrap_err()
            .to_string();
        assert!(message.contains(name));
        assert!(message.contains("解析配置文件"));
        assert!(message.contains(location));
        assert!(message.contains("请检查"));
        assert_error_redacted(message);
    }
}

#[test]
fn missing_and_wrong_type_paths_report_context_cause_and_repair() {
    let fixture = Fixture::new();
    let missing = fixture.target("missing-source");
    let message = RuntimeConfig::capture(&missing).unwrap_err().to_string();
    assert!(message.contains(missing.to_str().unwrap()));
    assert!(message.contains("检查配置目录"));
    assert!(message.contains("error.kind=NotFound"));
    assert!(message.contains("raw_os_error="));
    assert!(message.contains("请创建"));
    assert_error_redacted(message);

    let file = fixture.target("file-source");
    fs::write(&file, "must-not-echo").unwrap();
    let message = RuntimeConfig::capture(&file).unwrap_err().to_string();
    assert!(message.contains(file.to_str().unwrap()));
    assert!(message.contains("实际为普通文件"));
    assert!(message.contains("请改用"));
    assert_error_redacted(message);

    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    let target = fixture.target("missing-parent/target");
    let message = config.snapshot_to(&target).unwrap_err().to_string();
    assert!(message.contains(target.to_str().unwrap()));
    assert!(message.contains("准备新配置目标"));
    assert!(message.contains("error.kind=NotFound"));
    assert!(message.contains("raw_os_error="));
    assert!(message.contains("请创建"));
    assert!(!target.exists());
    assert_error_redacted(message);

    let target = fixture.target("existing-target");
    fs::create_dir(&target).unwrap();
    let message = config
        .write_profile(&target, normal(true), &fixture.target("locales"))
        .unwrap_err()
        .to_string();
    assert!(message.contains(target.to_str().unwrap()));
    assert!(message.contains("目标已存在"));
    assert!(message.contains("实际为目录"));
    assert!(message.contains("请指定新的目标目录"));
    assert_error_redacted(message);
}

#[test]
fn unknown_files_subdirectories_and_missing_dev_overlay_fail_closed() {
    for name in [
        "runtime.env",
        "secrets.toml",
        "APP_CONFIG_DIR",
        "must-not-echo",
    ] {
        let fixture = Fixture::new();
        fs::write(fixture.source.join(name), "must-not-echo").unwrap();
        assert_error_redacted(RuntimeConfig::capture(&fixture.source).unwrap_err());
    }
    let fixture = Fixture::new();
    fs::create_dir(fixture.source.join("nested")).unwrap();
    assert!(RuntimeConfig::capture(&fixture.source).is_err());
    let fixture = Fixture::new();
    fs::remove_file(fixture.source.join("app.dev.toml")).unwrap();
    let message = RuntimeConfig::capture(&fixture.source)
        .unwrap_err()
        .to_string();
    assert!(message.contains(fixture.source.join("app.dev.toml").to_str().unwrap()));
    assert!(message.contains("缺少必需文件"));
    assert!(message.contains("请创建 app.dev.toml"));
    assert_error_redacted(message);
    let fixture = Fixture::new();
    fs::remove_file(fixture.source.join("tools.toml")).unwrap();
    fs::create_dir(fixture.source.join("tools.toml")).unwrap();
    let message = RuntimeConfig::capture(&fixture.source)
        .unwrap_err()
        .to_string();
    assert!(message.contains(fixture.source.join("tools.toml").to_str().unwrap()));
    assert!(message.contains("实际为目录"));
    assert!(message.contains("请替换"));
    assert_error_redacted(message);
}

#[test]
fn snapshot_and_profile_refuse_existing_targets_without_modifying_them() {
    let fixture = Fixture::new();
    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    let target = fixture.target("snapshot");
    config.snapshot_to(&target).unwrap();
    let before = files_at(&target);
    assert!(config.snapshot_to(&target).is_err());
    assert!(
        config
            .write_profile(&target, normal(true), &fixture.target("locales"))
            .is_err()
    );
    assert_eq!(files_at(&target), before);
    let file = fixture.target("file");
    fs::write(&file, "untouched").unwrap();
    assert!(config.snapshot_to(&file).is_err());
    assert_eq!(fs::read_to_string(file).unwrap(), "untouched");
    assert!(
        config
            .snapshot_to(&fixture.target("missing-parent/target"))
            .is_err()
    );
    assert!(!fixture.target("missing-parent").exists());
}

#[test]
fn read_only_source_remains_byte_identical_after_snapshot_and_profile() {
    let fixture = Fixture::new();
    let before = files_at(&fixture.source);
    let mut permissions = Vec::new();
    for name in before.keys() {
        let path = fixture.source.join(name);
        let original = fs::metadata(&path).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        permissions.push((path, original));
    }
    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    config.snapshot_to(&fixture.target("snapshot")).unwrap();
    config
        .write_profile(
            &fixture.target("worker"),
            normal(true),
            &fixture.target("locales"),
        )
        .unwrap();
    assert_eq!(files_at(&fixture.source), before);
    for (path, original) in permissions {
        fs::set_permissions(path, original).unwrap();
    }
}

#[test]
fn normal_and_probe_roles_change_only_the_explicit_overlay_fields() {
    for (worker, probe) in [(false, false), (true, false), (false, true), (true, true)] {
        let fixture = Fixture::new();
        let source_before = files_at(&fixture.source);
        let config = RuntimeConfig::capture(&fixture.source).unwrap();
        let mut profile = normal(worker);
        profile.probe = probe;
        let target = fixture.target("role");
        let locales = fixture.target("最终 本地化/${APP_ENV}");
        config.write_profile(&target, profile, &locales).unwrap();
        let app = AppConfig::load(&target, Environment::Dev).unwrap();
        assert_eq!(app.app.port, 18080);
        assert_eq!(app.jobs.health_port, 19091);
        assert_eq!(app.snowflake_worker_id, 1023);
        assert_eq!(app.app.locales_dir, locales);
        assert_eq!(app.database.migration_mode, MigrationMode::Verify);
        assert_eq!(app.jobs.mode, JobWorkerMode::External);
        assert_eq!(app.app.name, "角色 测试");
        assert_eq!(app.database.primary.password, "overlay-db-private");
        assert_eq!(
            app.jobs.worker_id.as_deref(),
            Some(if probe {
                "ryframe-dev-probe"
            } else if worker {
                "ryframe-dev-worker"
            } else {
                "original-worker"
            })
        );
        assert_eq!(app.telemetry.enabled, !probe);
        assert_eq!(
            app.logger.output,
            if probe {
                LoggerOutput::Stdout
            } else {
                LoggerOutput::File
            }
        );
        assert_eq!(app.app.host, if probe { "127.0.0.1" } else { "0.0.0.0" });
        assert_eq!(
            app.jobs.health_host,
            if probe { "127.0.0.1" } else { "0.0.0.0" }
        );
        let mut expected = table_at(&fixture.source.join("app.dev.toml"));
        expected.insert("snowflake_worker_id".into(), 1023.into());
        set(&mut expected, "app", "port", 18080);
        set(
            &mut expected,
            "app",
            "locales_dir",
            locales.to_str().unwrap(),
        );
        set(&mut expected, "jobs", "health_port", 19091);
        set(&mut expected, "jobs", "mode", "external");
        set(&mut expected, "database", "migration_mode", "verify");
        if worker || probe {
            expected["jobs"]["worker_id"] = app.jobs.worker_id.unwrap().into();
        }
        if probe {
            set(&mut expected, "app", "host", "127.0.0.1");
            set(&mut expected, "jobs", "health_host", "127.0.0.1");
            expected["telemetry"]["enabled"] = false.into();
            expected["logger"]["output"] = "stdout".into();
        }
        assert_eq!(table_at(&target.join("app.dev.toml")), expected);
        for (name, bytes) in &source_before {
            if name != "app.dev.toml" {
                assert_eq!(&fs::read(target.join(name)).unwrap(), bytes);
            }
        }
        assert_eq!(files_at(&fixture.source), source_before);
    }
}

#[test]
fn invalid_ports_ids_and_locale_text_fail_before_creating_target() {
    let fixture = Fixture::new();
    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    let valid = normal(true);
    for invalid in [
        RuntimeProfile {
            api_port: 0,
            ..valid
        },
        RuntimeProfile {
            worker_port: 0,
            ..valid
        },
        RuntimeProfile {
            worker_port: valid.api_port,
            ..valid
        },
        RuntimeProfile {
            snowflake_worker_id: 1024,
            ..valid
        },
        RuntimeProfile {
            snowflake_worker_id: u16::MAX,
            ..valid
        },
    ] {
        let target = fixture.target("invalid");
        assert_error_redacted(
            config
                .write_profile(&target, invalid, &fixture.target("locales"))
                .unwrap_err(),
        );
        assert!(!target.exists());
    }
    for locales in [
        PathBuf::new(),
        PathBuf::from("locales"),
        fixture.target("must-not-echo\nlocales"),
        fixture.target("must-not-echo\rlocales"),
        fixture.target("must-not-echo\0locales"),
    ] {
        let target = fixture.target("invalid");
        assert_error_redacted(config.write_profile(&target, valid, &locales).unwrap_err());
        assert!(!target.exists());
    }
}

#[test]
fn boundary_ports_and_zero_id_are_valid_without_locale_filesystem_access() {
    let fixture = Fixture::new();
    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    let locales = fixture.target("not-created-locales");
    config
        .write_profile(
            &fixture.target("role"),
            RuntimeProfile {
                api_port: 1,
                worker_port: u16::MAX,
                snowflake_worker_id: 0,
                probe: true,
                worker: true,
            },
            &locales,
        )
        .unwrap();
    assert!(!locales.exists());
}

#[cfg(any(unix, windows))]
fn link(source: &Path, target: &Path, directory: bool) -> bool {
    #[cfg(unix)]
    let result = {
        let _ = directory;
        std::os::unix::fs::symlink(source, target)
    };
    #[cfg(windows)]
    let result = if directory {
        std::os::windows::fs::symlink_dir(source, target)
    } else {
        std::os::windows::fs::symlink_file(source, target)
    };
    match result {
        Ok(()) => true,
        #[cfg(windows)]
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("无法创建测试链接：{error}"),
    }
}

#[cfg(any(unix, windows))]
#[test]
fn file_directory_and_ancestor_links_are_rejected_including_dangling_targets() {
    let fixture = Fixture::new();
    let config = RuntimeConfig::capture(&fixture.source).unwrap();
    let outside = fixture.target("outside.toml");
    fs::write(&outside, "[app]\n").unwrap();
    let entry = fixture.source.join("tools.toml");
    fs::remove_file(&entry).unwrap();
    if !link(&outside, &entry, false) {
        return;
    }
    assert!(RuntimeConfig::capture(&fixture.source).is_err());
    fs::remove_file(&entry).unwrap();
    fs::write(entry, "[generator]\n").unwrap();
    let alias = fixture.target("alias");
    assert!(link(&fixture.source, &alias, true));
    assert!(RuntimeConfig::capture(&alias).is_err());
    assert!(RuntimeConfig::capture(&alias.join("../source")).is_err());
    assert!(config.snapshot_to(&alias.join("new-target")).is_err());
    assert!(
        config
            .write_profile(
                &alias.join("new-target"),
                normal(true),
                &fixture.target("locales")
            )
            .is_err()
    );
    assert!(!fixture.source.join("new-target").exists());
    let dangling = fixture.target("dangling");
    assert!(link(&fixture.target("missing"), &dangling, true));
    assert!(config.snapshot_to(&dangling).is_err());
    assert!(
        config
            .write_profile(&dangling, normal(true), &fixture.target("locales"))
            .is_err()
    );
    assert!(fs::symlink_metadata(dangling).is_ok());
}
