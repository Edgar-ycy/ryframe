use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

#[path = "../src/bin/ryframe_migrate/bootstrap.rs"]
mod bootstrap;
#[path = "boot_contract/configuration_fixture.rs"]
mod configuration_fixture;

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
}

fn run(output: &Path, mode: &str, password: Option<&str>) -> Output {
    run_from(workspace_root(), output, mode, password)
}

fn run_from(root: &Path, output: &Path, mode: &str, password: Option<&str>) -> Output {
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
    let root = workspace_root().join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("migration-cli-")
        .tempdir_in(root)
        .unwrap()
}

fn external_directory() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("migration-cli-external-")
        .tempdir_in(workspace_root().parent().unwrap())
        .unwrap()
}

#[test]
fn sql_plan_does_not_read_secrets_or_create_output_directories() {
    let directory = external_directory();
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
fn ownership_plan_is_offline_and_separate_from_business_adoption() {
    let directory = directory();
    let missing = directory.path().join("missing-config");
    let output = directory.path().join("missing-output/plan.json");
    let result = Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"))
        .arg("--config-dir")
        .arg(&missing)
        .args(["control", "ownership-adopt", "--plan", "--output"])
        .arg(&output)
        .env("APP_ENV", "invalid-environment")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!missing.exists() && !output.parent().unwrap().exists());
    let mixed = Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"))
        .arg("--config-dir")
        .arg(&missing)
        .args(["control", "ownership-adopt", "--module", "order", "--plan"])
        .output()
        .unwrap();
    assert!(!mixed.status.success());
    assert!(!missing.exists());
}

#[test]
fn export_requires_explicit_password_and_refuses_overwrite() {
    let directory = external_directory();
    let output = directory.path().join("new-parent/init.sql");
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
    assert_eq!(fs::read_dir(output.parent().unwrap()).unwrap().count(), 1);
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

#[test]
fn sql_export_refuses_other_checkouts_before_creating_directories() {
    let directory = external_directory();
    for marker in ["repository", "linked-worktree"] {
        let checkout = directory.path().join(marker);
        fs::create_dir(&checkout).unwrap();
        if marker == "repository" {
            fs::create_dir(checkout.join(".git")).unwrap();
        } else {
            fs::write(checkout.join(".git"), "gitdir: linked-checkout\n").unwrap();
        }
        let output = checkout.join("new-parent/init.sql");
        let result = run(&output, "--write", Some("InitialAdmin123!"));
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(error.contains("Git 检出目录之外"), "{error}");
        assert!(!output.parent().unwrap().exists());
    }
}

#[test]
fn sql_export_refuses_the_current_source_tree_without_git_metadata() {
    let directory = external_directory();
    let root = directory.path();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir_all(root.join("crates/ryframe")).unwrap();
    let output = root.join("new-parent/init.sql");
    let result = run_from(root, &output, "--write", Some("InitialAdmin123!"));
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(error.contains("当前后端源码树之外"), "{error}");
    assert!(!output.parent().unwrap().exists());
}

#[test]
fn sql_export_refuses_an_ordinary_relative_output() {
    let directory = directory();
    let output = Path::new(directory.path().file_name().unwrap()).join("new-parent/init.sql");
    let result = run(&output, "--write", Some("InitialAdmin123!"));
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        error.contains("私有文件须使用检出目录之外的绝对路径"),
        "{error}"
    );
    assert!(!workspace_root().join(output).parent().unwrap().exists());
}

#[cfg(any(unix, windows))]
#[test]
fn sql_export_refuses_linked_external_parents_and_existing_directories() {
    let directory = external_directory();
    let ordinary = directory.path().join("ordinary");
    fs::create_dir(&ordinary).unwrap();
    let link = directory.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&ordinary, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&ordinary, &link).unwrap();
    let result = run(
        &link.join("new-parent/init.sql"),
        "--write",
        Some("InitialAdmin123!"),
    );
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(
        error.contains("符号链接") || error.contains("重解析点"),
        "{error}"
    );
    assert!(!ordinary.join("new-parent").exists());

    let result = run(&ordinary, "--write", Some("InitialAdmin123!"));
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(error.contains("目标文件已存在"), "{error}");
    assert_eq!(fs::read_dir(&ordinary).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn sql_export_accepts_verbatim_private_paths_and_preserves_namespace_guards() {
    use std::path::{Component, PathBuf, Prefix};

    let directory = directory();
    let verbatim = fs::canonicalize(directory.path()).unwrap();
    assert!(
        matches!(verbatim.components().next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::VerbatimDisk(_) | Prefix::VerbatimUNC(..)))
    );
    let spelling = verbatim.to_str().unwrap();
    let ordinary = if let Some(unc) = spelling.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(spelling.strip_prefix(r"\\?\").unwrap())
    };
    assert!(
        matches!(ordinary.components().next(), Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::UNC(..)))
    );
    let output = verbatim.join("init.sql");
    let result = run(&output, "--write", Some("VerbatimExport-Test-Admin42!"));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let original = fs::read(&output).unwrap();
    assert!(!original.is_empty());
    let repeated = run(
        &ordinary.join("init.sql"),
        "--write",
        Some("AnotherAdmin123!"),
    );
    assert!(!repeated.status.success());
    assert_eq!(fs::read(&output).unwrap(), original);

    let unique = directory.path().file_name().unwrap().to_str().unwrap();
    // 直接构造原始拼写，避免 PathBuf::push 在 verbatim 路径上消去父级组件。
    let parent_escape = PathBuf::from(format!(
        r"{}\..\{unique}-parent-init.sql",
        verbatim.display()
    ));
    assert!(
        parent_escape
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    );
    let escaped_target = directory
        .path()
        .parent()
        .unwrap()
        .join(format!("{unique}-parent-init.sql"));
    assert!(!escaped_target.exists());
    let result = run(
        &parent_escape,
        "--write",
        Some("VerbatimExport-Test-Admin42!"),
    );
    assert!(!result.status.success());
    assert!(!escaped_target.exists());

    let outside_parent = verbatim
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("{unique}-outside-private"));
    let outside = outside_parent.join("init.sql");
    assert!(
        !outside
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    );
    assert!(!outside_parent.exists());
    let result = run(&outside, "--write", Some("VerbatimExport-Test-Admin42!"));
    assert!(!result.status.success());
    assert!(!outside_parent.exists());
    let case_alias = PathBuf::from(outside.display().to_string().to_ascii_uppercase());
    let result = run(&case_alias, "--write", Some("VerbatimExport-Test-Admin42!"));
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(error.contains("当前后端源码树之外"), "{error}");
    assert!(!outside_parent.exists());
    assert_eq!(fs::read(&output).unwrap(), original);
}

fn adoption(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"))
        .current_dir(workspace_root())
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
    let marker = directory.path().join(".git");
    fs::write(&marker, "gitdir: fixture-checkout\n").unwrap();
    let absolute = directory.path().join("bad-plan.json");
    let path = absolute.strip_prefix(workspace_root()).unwrap();
    for malformed in ["{invalid", r#"{"manifest":"PRIVATE-PLAN-VALUE"}"#] {
        fs::write(&absolute, malformed).unwrap();
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
        assert_eq!(fs::read_to_string(&absolute).unwrap(), malformed);
        assert!(!error.contains("PRIVATE-PLAN-VALUE"));
        assert!(error.contains("阶段=读取原接管计划"));
    }
    assert_eq!(
        fs::read_to_string(marker).unwrap(),
        "gitdir: fixture-checkout\n"
    );
}

#[test]
fn adoption_reads_explicit_external_plans_before_runtime_configuration() {
    let directory = external_directory();
    let path = directory.path().join("bad-plan.json");
    let body = "{invalid";
    fs::write(&path, body).unwrap();
    for (command, diagnostic) in [("adopt", "格式错误"), ("ownership-adopt", "at line")] {
        let result = adoption(&["control", command, "--inspect", path.to_str().unwrap()]);
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(error.contains(diagnostic), "{error}");
        assert!(!error.contains("must-not-read-for-plan"), "{error}");
        assert_eq!(fs::read_to_string(&path).unwrap(), body);
    }
    let result = adoption(&[
        "control",
        "adopt",
        "--inspect",
        directory.path().to_str().unwrap(),
    ]);
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(error.contains("私有计划必须是普通文件"), "{error}");
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
            assert!(
                error.contains("数据库连接失败"),
                "未传播真实底层连接阶段：{error}"
            );
            assert!(error.contains(" -> "), "未消费内部原因链：{error}");
        }
    }
}
