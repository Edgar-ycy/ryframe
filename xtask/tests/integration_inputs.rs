use std::{fs, path::Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[path = "../src/ci/integration_inputs.rs"]
mod integration_inputs;

fn fixture() -> tempfile::TempDir {
    let parent =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.local-tests/integration-inputs-tests");
    fs::create_dir_all(&parent).unwrap();
    tempfile::tempdir_in(parent).unwrap()
}

fn write_inputs(root: &Path) {
    for directory in [
        root.join(".local-tests/mysql/config"),
        root.join(".local-tests/service-tests/redis/config"),
    ] {
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("app.toml"),
            include_str!("../../config/app.toml"),
        )
        .unwrap();
        fs::write(directory.join("app.test.toml"), selected()).unwrap();
    }
}

fn selected() -> String {
    format!(
        "scope_id = \"integration-input-test\"\nsnowflake_worker_id = 1\n{}",
        include_str!("../../tools/fixtures/integration/app.test.toml")
    )
}

#[test]
fn missing_inputs_fail_before_any_service_or_directory_is_created() {
    let root = fixture();
    let error = integration_inputs::require_service_inputs(root.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("IOkind=NotFound") && error.contains("本次尚未运行服务测试"));
    assert!(error.contains(root.path().to_str().unwrap()));
    assert!(fs::read_dir(root.path()).unwrap().next().is_none());
}

#[test]
fn both_selected_files_are_required_even_when_common_defaults_exist() {
    let root = fixture();
    write_inputs(root.path());
    for role in ["mysql", "service-tests/redis"] {
        let file = root
            .path()
            .join(".local-tests")
            .join(role)
            .join("config/app.test.toml");
        fs::remove_file(&file).unwrap();
        let error = integration_inputs::require_service_inputs(root.path())
            .unwrap_err()
            .to_string();
        assert!(error.contains("app.test.toml") && error.contains("本次尚未运行服务测试"));
        assert!(!file.exists());
        fs::write(&file, selected()).unwrap();
    }
}

#[test]
fn malformed_input_does_not_echo_credentials_or_repair_the_source() {
    let root = fixture();
    write_inputs(root.path());
    let file = root.path().join(".local-tests/mysql/config/app.test.toml");
    let secret = "private-input-value-do-not-echo";
    let bytes = format!("password = \"{secret}");
    fs::write(&file, &bytes).unwrap();
    let error = integration_inputs::require_service_inputs(root.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("app.test.toml") && error.contains("本次尚未运行服务测试"));
    assert!(!error.contains(secret));
    assert_eq!(fs::read_to_string(&file).unwrap(), bytes);
}

#[test]
fn explicit_gate_rejects_disabled_mysql_tls_and_missing_redis() {
    let root = fixture();
    write_inputs(root.path());
    let mysql = root.path().join(".local-tests/mysql/config/app.test.toml");
    fs::write(
        &mysql,
        selected().replace("tls_mode = \"required\"", "tls_mode = \"disabled\""),
    )
    .unwrap();
    let error = integration_inputs::require_service_inputs(root.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("database.primary.tls_mode") && error.contains("本次尚未运行服务测试"));
    fs::write(&mysql, selected()).unwrap();
    let redis = root.path().join(".local-tests/service-tests/redis/config");
    for name in ["app.toml", "app.test.toml"] {
        let file = redis.join(name);
        let mut table: toml::Table = fs::read_to_string(&file).unwrap().parse().unwrap();
        table.remove("redis");
        fs::write(file, toml::to_string(&table).unwrap()).unwrap();
    }
    let error = integration_inputs::require_service_inputs(root.path())
        .unwrap_err()
        .to_string();
    assert!(error.contains("缺少 redis 分组") && error.contains("本次尚未运行服务测试"));
}

#[test]
fn valid_isolated_files_are_verified_without_writes_or_connections() {
    let root = fixture();
    write_inputs(root.path());
    let files = [
        ".local-tests/mysql/config/app.toml",
        ".local-tests/mysql/config/app.test.toml",
        ".local-tests/service-tests/redis/config/app.toml",
        ".local-tests/service-tests/redis/config/app.test.toml",
    ];
    let before = files.map(|file| fs::read(root.path().join(file)).unwrap());
    integration_inputs::require_service_inputs(root.path()).unwrap();
    let after = files.map(|file| fs::read(root.path().join(file)).unwrap());
    assert_eq!(before, after);
    assert!(!root.path().join("storage").exists());
}
