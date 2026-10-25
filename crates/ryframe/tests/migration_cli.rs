use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

#[path = "../src/bin/ryframe_migrate/bootstrap.rs"]
mod bootstrap;
#[path = "boot_contract/configuration_fixture.rs"]
mod configuration_fixture;

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
    fs::write(&path, "{invalid").unwrap();
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
    assert_eq!(fs::read_to_string(path).unwrap(), "{invalid");
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
