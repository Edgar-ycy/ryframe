use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

#[path = "../src/bin/ryframe_migrate/bootstrap.rs"]
mod bootstrap;
#[path = "boot_contract/configuration_fixture.rs"]
mod configuration_fixture;
#[path = "../src/bin/ryframe_migrate/diagnostic.rs"]
mod diagnostic;

fn run(output: &Path, mode: &str, password: Option<&str>) -> Output {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let configuration = directory();
    let config = configuration.path().join("config");
    configuration_fixture::write_configuration(&config, 8080, 9091, password);
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"));
    command
        .current_dir(root)
        .arg("--config-dir")
        .arg(&config)
        .args(["control", "export-sql", "--output"])
        .arg(output)
        .arg(mode)
        .env(
            "APP_ENV",
            if mode == "--plan" {
                "not-a-runtime-environment"
            } else {
                "test"
            },
        );
    command.output().unwrap()
}

fn directory() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("migration-cli-")
        .tempdir_in(root)
        .unwrap()
}

#[test]
fn sql_plan_does_not_read_secrets_or_create_output_directories() {
    let directory = directory();
    let output = directory.path().join("not-created/init.sql");
    let result = run(&output, "--plan", Some("invalid"));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!output.parent().unwrap().exists());
}

#[test]
fn export_requires_explicit_password_and_refuses_overwrite() {
    let directory = directory();
    let output = directory.path().join("init.sql");
    let missing = run(&output, "--write", None);
    assert!(!missing.status.success());
    assert!(!output.exists());
    let result = run(&output, "--write", Some("InitialAdmin123!"));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let sql = fs::read_to_string(&output).unwrap();
    assert!(!sql.contains("InitialAdmin123!"));
    assert!(sql.contains("$argon2id$"));
    assert!(sql.contains("CREATE TABLE `seaql_migrations`"));
    assert!(sql.contains("CREATE TABLE `seaql_tenant_data_migrations`"));
    assert!(sql.contains("INSERT INTO `biz_tenant_fence`"));
    assert!(sql.contains("INSERT INTO `biz_tenant_target_slot`"));
    assert_eq!(
        sql.matches("CREATE TABLE `ryframe_resource_ownership`")
            .count(),
        1
    );
    assert!(!sql.contains("CREATE TABLE IF NOT EXISTS"));
    let repeated = run(&output, "--write", Some("AnotherAdmin123!"));
    assert!(!repeated.status.success());
    assert_eq!(fs::read_to_string(&output).unwrap(), sql);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn sql_export_refuses_escape_from_private_directory() {
    let directory = directory();
    let output = directory.path().join("../escaped-init.sql");
    let result = run(&output, "--write", Some("InitialAdmin123!"));
    assert!(!result.status.success());
    assert!(
        !directory
            .path()
            .parent()
            .unwrap()
            .join("escaped-init.sql")
            .exists()
    );
}

fn adoption(args: &[&str]) -> Output {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"))
        .current_dir(root)
        .args(args)
        .args(["--config-dir", "must-not-read-for-plan"])
        .env("APP_ENV", "must-not-read-for-plan")
        .output()
        .unwrap()
}

#[test]
fn adoption_plans_do_not_read_config_secrets_or_input_and_do_not_create_output() {
    let directory = directory();
    let output = directory.path().join("never-create/adopt.json");
    let path = output.to_str().unwrap();
    for args in [
        vec!["control", "adopt", "--module", "order", "--plan"],
        vec![
            "control", "adopt", "--module", "order", "--output", path, "--plan",
        ],
        vec!["control", "adopt", "--apply", path, "--plan"],
        vec!["control", "adopt", "--inspect", path, "--plan"],
        vec![
            "tenant-data",
            "adopt",
            "--module",
            "order",
            "--target",
            "business-shared",
            "--plan",
        ],
    ] {
        let result = adoption(&args);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("任务图"));
        assert!(!output.parent().unwrap().exists());
    }
}

#[test]
fn adoption_rejects_ambiguous_scopes_missing_values_and_implicit_writes() {
    for args in [
        vec!["control", "adopt", "--module", "order", "--write"],
        vec![
            "control",
            "adopt",
            "--module",
            "order",
            "--output",
            "unused.json",
        ],
        vec!["control", "adopt", "--apply", "unused.json"],
        vec!["control", "adopt", "--inspect", "unused.json", "--write"],
        vec![
            "control",
            "adopt",
            "--module",
            "order",
            "--target",
            "shared-control",
            "--plan",
        ],
        vec!["tenant-data", "adopt", "--module", "order", "--plan"],
        vec![
            "tenant-data",
            "adopt",
            "--module",
            "order",
            "--all",
            "--plan",
        ],
        vec!["control", "adopt", "--module", "--plan"],
        vec![
            "control", "adopt", "--module", "order", "--module", "other", "--plan",
        ],
        vec![
            "control",
            "adopt",
            "--inspect",
            "unused.json",
            "--module",
            "order",
            "--plan",
        ],
        vec![
            "control",
            "adopt",
            "--apply",
            "unused.json",
            "--write",
            "--plan",
        ],
    ] {
        let result = adoption(&args);
        assert!(!result.status.success(), "意外接受 {args:?}");
        assert!(!String::from_utf8_lossy(&result.stderr).contains("must-not-read-for-plan"));
    }
}

#[test]
fn malformed_adoption_plan_fails_before_loading_runtime_configuration() {
    let directory = directory();
    let path = directory.path().join("bad-plan.json");
    for malformed in ["{invalid", r#"{"manifest":"PRIVATE-PLAN-VALUE"}"#] {
        fs::write(&path, malformed).unwrap();
        let result = adoption(&[
            "control",
            "adopt",
            "--apply",
            path.to_str().unwrap(),
            "--write",
        ]);
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("格式错误"), "{error}");
        assert!(!error.contains("must-not-read-for-plan"));
        assert_eq!(fs::read_to_string(&path).unwrap(), malformed);
        assert!(!error.contains("PRIVATE-PLAN-VALUE"));
        assert!(error.contains("阶段=读取原接管计划"));
    }
}

#[test]
fn configuration_arguments_fail_before_plan_or_runtime_access() {
    for args in [
        vec![
            "control",
            "adopt",
            "--module",
            "order",
            "--plan",
            "--config-dir",
            "other",
        ],
        vec![
            "control",
            "adopt",
            "--module",
            "order",
            "--plan",
            "--config-dir",
        ],
        vec![
            "control",
            "adopt",
            "--module",
            "order",
            "--plan",
            "--config-dir=",
        ],
    ] {
        let result = adoption(&args);
        assert!(!result.status.success());
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("--config-dir"), "{error}");
        assert!(!error.contains("APP_ENV"), "{error}");
        assert!(
            error.contains("cargo migrate -- --config-dir <当前已显示配置目录> control status")
        );
        assert!(!error.contains("cargo migrate -- control"));
        assert!(!error.contains("或 tenant-data"));
    }
}

#[test]
fn invalid_configured_password_never_appears_in_export_diagnostics_or_output() {
    let directory = directory();
    let output = directory.path().join("init.sql");
    let password = "private-but-invalid-password";
    let result = run(&output, "--write", Some(password));
    assert!(!result.status.success());
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("config.bootstrap.admin_password"), "{error}");
    assert!(!error.contains(password));
    assert!(!String::from_utf8_lossy(&result.stdout).contains(password));
    assert!(
        error.contains("cargo migrate -- --config-dir <当前已显示配置目录> control export-sql")
    );
    assert!(!error.contains("cargo migrate -- control"));
    assert!(!output.exists());
}

#[test]
fn ordinary_migration_modes_use_the_explicit_directory_before_any_connection() {
    let directory = directory();
    let missing = directory.path().join("selected-but-absent");
    for scope in ["control", "tenant-data"] {
        for operation in ["up", "verify", "status"] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"));
            command
                .args([scope, operation, "--config-dir"])
                .arg(&missing)
                .env("APP_ENV", "test");
            if scope == "tenant-data" {
                command.args(["--target", "shared-control"]);
            }
            let output = command.output().unwrap();
            assert!(!output.status.success());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(error.contains("selected-but-absent"), "{error}");
            assert!(!error.contains("用法"), "{error}");
            assert!(!missing.exists());
        }
    }
}

#[test]
fn bootstrap_allows_existing_database_without_password_and_keeps_initial_user_hash() {
    use ryframe_config::{AppConfig, Environment};

    let directory = directory();
    for password in [None, Some(""), Some("  ")] {
        configuration_fixture::write_configuration(directory.path(), 8080, 9091, password);
        let config = AppConfig::load(directory.path(), Environment::Test).unwrap();
        assert!(bootstrap::load_hashes(&config).unwrap().is_none());
    }
    configuration_fixture::write_configuration(
        directory.path(),
        8080,
        9091,
        Some("InitialAdmin123!"),
    );
    let config = AppConfig::load(directory.path(), Environment::Test).unwrap();
    let hashes = bootstrap::load_hashes(&config).unwrap().unwrap();
    let sql = ryframe_db::migration::mysql_initialization_sql(&hashes);
    let hashes = sql
        .split('\'')
        .filter(|value| value.starts_with("$argon2id$"))
        .collect::<Vec<_>>();
    assert!(
        hashes
            .iter()
            .any(|hash| ryframe_auth::password::verify("InitialAdmin123!", hash).unwrap())
    );
    assert!(hashes.iter().any(|hash| {
        ryframe_auth::password::verify(ryframe_auth::password::INITIAL_USER_PASSWORD, hash).unwrap()
    }));
    assert!(!sql.contains("InitialAdmin123!"));
}

fn write_unreachable_database_configuration(config: &Path, port: u16) {
    configuration_fixture::write_configuration(config, 8080, 9091, None);
    let base = fs::read_to_string(config.join("app.toml"))
        .unwrap()
        .replace("port = 1\n", &format!("port = {port}\n"))
        .replace(
            "min_connections = 0",
            "min_connections = 1\nconnect_timeout_secs = 1\nacquire_timeout_secs = 1",
        );
    fs::write(config.join("app.toml"), base).unwrap();
    fs::write(
        config.join("app.test.toml"),
        format!(
            r#"
[[tenant_data.targets]]
key = "shared-control"
mode = "shared"
kind = "control"
[[tenant_data.targets]]
key = "diagnostic-mysql"
mode = "dedicated"
kind = "mysql"
host = "127.0.0.1"
port = {port}
database = "diagnostic_tenant_only"
username = "private-tenant-user"
password = "private-tenant-password"
"#
        ),
    )
    .unwrap();
    let loaded =
        ryframe_config::AppConfig::load(config, ryframe_config::Environment::Test).unwrap();
    assert!(
        loaded
            .tenant_data
            .targets
            .iter()
            .any(|target| target.key == "shared-control"
                && target.kind == ryframe_config::TenantDatabaseTargetKind::Control)
    );
    assert!(
        loaded
            .tenant_data
            .targets
            .iter()
            .filter_map(|target| target.database.as_deref())
            .all(|database| database != loaded.database.primary.database.as_str())
    );
}

#[test]
fn migration_connection_failures_keep_phase_files_and_safe_repair_without_writes() {
    let root = directory();
    let config = root.path().join("config");
    // 只保留测试自己的 TCP 监听，不提供 MySQL 握手，任何命令都不可能发出 SQL。
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    write_unreachable_database_configuration(&config, port);
    for (scope, operation, target, phase, field) in [
        ("control", "up", None, "连接池建立", "database.primary"),
        ("control", "verify", None, "连接池建立", "database.primary"),
        ("control", "status", None, "连接池建立", "database.primary"),
        (
            "tenant-data",
            "up",
            Some("shared-control"),
            "目标连接预检",
            "database.primary",
        ),
        (
            "tenant-data",
            "verify",
            Some("diagnostic-mysql"),
            "独立目标连接预检",
            "tenant_data.targets",
        ),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"));
        command
            .args([scope, operation, "--config-dir"])
            .arg(&config)
            .env("APP_ENV", "test");
        if let Some(target) = target {
            command.args(["--target", target]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        for expected in [
            phase,
            field,
            "app.test.toml",
            "原因链=",
            "当前命令尚未发起",
            "cargo migrate --",
            "tls_client_key",
        ] {
            assert!(error.contains(expected), "缺少 {expected}：{error}");
        }
        for secret in [
            "fixture-database-password",
            "private-tenant-user",
            "private-tenant-password",
        ] {
            assert!(!error.contains(secret), "敏感输入泄露");
        }
        for (suggestion, _) in error.match_indices("cargo migrate --") {
            assert!(
                error[suggestion..]
                    .starts_with("cargo migrate -- --config-dir <当前已显示配置目录>")
            );
        }
        assert!(!error.contains("Error: Database("));
        assert!(!error.contains("1045"));
        if target == Some("diagnostic-mysql") {
            assert!(error.contains("未保留底层原因"));
        }
    }
}

#[test]
fn operation_and_close_failures_keep_both_stages_and_conservative_write_state() {
    let root = directory();
    let selected = root.path().join("config");
    configuration_fixture::write_configuration(&selected, 8080, 9091, None);
    let config =
        ryframe_config::AppConfig::load(&selected, ryframe_config::Environment::Test).unwrap();
    let context = diagnostic::Context {
        directory: &selected,
        environment: Some(config.environment),
        config: Some(&config),
        command: "control up",
        target: None,
    };
    let before = context
        .error(
            "连接池建立",
            diagnostic::Writes::None,
            &std::io::Error::other("connect failed"),
        )
        .to_string();
    assert!(before.contains("当前命令尚未发起"));
    assert!(!before.contains("可能已部分提交"));
    let file = context
        .error(
            "文件发布",
            diagnostic::Writes::File,
            &std::io::Error::other("publish failed"),
        )
        .to_string();
    assert!(file.contains("目录、临时文件或目标文件可能已创建"));
    assert!(!file.contains("尚未发起"));
    let operation = context.error(
        "业务迁移执行",
        diagnostic::Writes::Database,
        &std::io::Error::other("operation failed fixture-database-password"),
    );
    let close = context.error(
        "关闭连接",
        diagnostic::Writes::Database,
        &std::io::Error::other("close failed mysql://unknown:unlisted@localhost/db"),
    );
    let error = diagnostic::finish::<()>(Err(operation), Err(close))
        .unwrap_err()
        .to_string();
    for expected in [
        "业务迁移执行",
        "operation failed",
        "关闭连接",
        "close failed",
        "DDL 或账本可能已部分提交",
        "禁止自动重放",
    ] {
        assert!(error.contains(expected), "{error}");
    }
    assert!(!error.contains("fixture-database-password"));
    assert!(!error.contains("unknown:unlisted"));
    assert!(!error.contains("://"));
    let adoption = diagnostic::Context {
        command: "control adopt --apply",
        ..context
    };
    let error = adoption
        .error(
            "接管账本应用",
            diagnostic::Writes::Database,
            &std::io::Error::other("apply failed"),
        )
        .to_string();
    assert!(
        error
            .contains("cargo migrate -- --config-dir <当前已显示配置目录> control adopt --inspect")
    );
    assert!(!error.contains("cargo migrate -- control"));
    assert!(!error.contains("或 tenant-data"));
}
