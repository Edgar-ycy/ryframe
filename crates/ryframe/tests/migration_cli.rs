use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(output: &Path, mode: &str, password: Option<&str>) -> Output {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ryframe-migrate"));
    command
        .current_dir(root)
        .args(["control", "export-sql", "--output"])
        .arg(output)
        .arg(mode)
        .env("APP_ENV", "not-a-runtime-environment")
        .env_remove("RYFRAME_INIT_ADMIN_PASSWORD_FILE")
        .env_remove("RYFRAME_INIT_ADMIN_PASSWORD");
    if let Some(password) = password {
        command.env("RYFRAME_INIT_ADMIN_PASSWORD", password);
    }
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
