mod support;

use ryframe_config::{BootstrapConfig, Environment, TenantDataConfig};
use support::{Fixture, PRODUCTION};

#[test]
fn removed_environment_entries_cannot_override_file_configuration_in_a_subprocess() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "removed_environment_probe_uses_its_own_files",
            "--nocapture",
        ])
        .env("APP_ENV", "dev")
        .env("APP_APP_PORT", "obsolete-port-not-an-integer")
        .env("APP_AUTH_JWT_SECRET", "obsolete-jwt-must-not-be-used")
        .env(
            "APP_AUTH_JWT_SECRET_FILE",
            "obsolete-secret-file-must-not-be-opened",
        )
        .env(
            "APP_CONFIG_DIR",
            "obsolete-config-directory-must-not-be-opened",
        )
        .env("SNOWFLAKE_WORKER_ID", "obsolete-worker-not-an-integer")
        .env("APP_TENANT_DATA_TARGETS", "obsolete-targets-not-json")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "子进程文件配置探针失败：{output:?}"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("test result: ok. 1 passed"), "{stdout}");
}

#[test]
#[ignore = "仅由父用例在注入已退役配置变量的独立子进程中执行"]
fn removed_environment_probe_uses_its_own_files() {
    let fixture = Fixture::new();
    fixture.write(
        Environment::Dev,
        r#"
snowflake_worker_id = 23
[app]
port = 8234
[auth]
jwt_secret = "file-only-jwt-secret"
"#,
    );
    let config = fixture.load(Environment::Dev);
    assert_eq!(config.app.port, 8234);
    assert_eq!(config.snowflake_worker_id, 23);
    assert_eq!(config.auth.jwt_secret, "file-only-jwt-secret");
    assert!(config.tenant_data.targets.is_empty());
}

#[test]
fn unknown_fields_and_wrong_types_report_file_and_field_without_values() {
    let fixture = Fixture::new();
    for (source, field) in [
        (
            "[bootstrap]\nadmin_pasword = 'private-unknown-value'",
            "admin_pasword",
        ),
        (
            "[bootstrap]\nadmin_password = ['private-array-value']",
            "bootstrap.admin_password",
        ),
        (
            "snowflake_worker_id = 'private-worker-value'",
            "snowflake_worker_id",
        ),
        (
            "[auth]\njwt_secret = ['private-key-value']",
            "auth.jwt_secret",
        ),
        ("[redis]\nmode = 'private-invalid-enum-value'", "redis.mode"),
        ("[redis]\nmode = 'private-quoted-\\\"value'", "redis.mode"),
    ] {
        fixture.write(Environment::Dev, source);
        let error = fixture.error(Environment::Dev);
        assert!(error.contains("app.dev.toml"), "{error}");
        assert!(error.contains(field), "{error}");
        assert!(!error.contains("private-"), "{error}");
        assert!(error.contains("修正"));
    }
}

#[test]
fn malformed_and_non_utf8_files_do_not_echo_original_lines() {
    let fixture = Fixture::new();
    fixture.write(
        Environment::Dev,
        "[bootstrap]\nadmin_password = \"private-unclosed-secret",
    );
    let error = fixture.error(Environment::Dev);
    assert!(error.contains("app.dev.toml"));
    assert!(error.contains("TOML 语法错误"));
    assert!(error.contains("第 2 行"));
    assert!(!error.contains("private-unclosed-secret"));
    std::fs::write(fixture.directory.join("app.dev.toml"), [0xff, 0xfe]).unwrap();
    assert!(fixture.error(Environment::Dev).contains("UTF-8"));
}

#[test]
fn removed_encoding_is_rejected_with_plaintext_guidance_without_secret_echo() {
    let fixture = Fixture::new();
    fixture.write(
        Environment::Dev,
        "[bootstrap]\nadmin_password = 'ENC[private-value]'",
    );
    let error = fixture.error(Environment::Dev);
    assert!(error.contains("bootstrap.admin_password"));
    assert!(error.contains("明文"));
    assert!(!error.contains("private-value"));
    assert!(!error.contains("secret manager"));
}

#[test]
fn bootstrap_password_is_optional_for_runtime_and_always_redacted_in_debug() {
    let fixture = Fixture::new();
    for password in ["", "short", "private-initial-admin-password"] {
        fixture.write(
            Environment::Dev,
            &format!("[bootstrap]\nadmin_password = '{password}'"),
        );
        let config = fixture.load(Environment::Dev);
        assert_eq!(config.bootstrap.admin_password.as_deref(), Some(password));
        assert!(!format!("{config:?}").contains("private-initial-admin-password"));
        assert!(format!("{:?}", config.bootstrap).contains("<redacted>"));
    }
    let absent = BootstrapConfig::default();
    assert!(absent.admin_password.is_none());
}

#[test]
fn mysql_targets_require_direct_password_but_allow_empty_password() {
    let template = r#"
[[targets]]
key = "tenant-a"
mode = "dedicated"
kind = "mysql"
host = "127.0.0.1"
database = "tenant_a"
username = "tenant_user"
"#;
    let missing: TenantDataConfig = toml::from_str(template).unwrap();
    assert!(missing.validate().unwrap_err().contains("password"));
    for password in ["", "private-tenant-password"] {
        let config: TenantDataConfig =
            toml::from_str(&format!("{template}\npassword = '{password}'")).unwrap();
        config.validate().unwrap();
        assert_eq!(config.targets[0].password.as_deref(), Some(password));
        assert!(!format!("{:?}", config.targets[0]).contains("private-tenant-password"));
    }
    assert!(
        toml::from_str::<TenantDataConfig>(&format!("{template}\npassword_env = 'REMOVED'"))
            .is_err()
    );
}

#[test]
fn control_target_rejects_even_an_empty_password_connection_field() {
    let config: TenantDataConfig = toml::from_str(
        r#"
[[targets]]
key = "shared-control"
mode = "shared"
kind = "control"
password = ""
"#,
    )
    .unwrap();
    assert!(
        config
            .validate()
            .unwrap_err()
            .contains("不得声明 MySQL 连接字段")
    );
}

#[test]
fn tls_scope_and_tenant_pool_constraints_remain_effective() {
    let fixture = Fixture::new();
    for (overlay, field) in [
        (
            "[database.primary]\nhost = 'remote.example.com'\ntls_mode = 'disabled'",
            "tls_mode",
        ),
        (
            "[redis]\nmode = 'required'\nhost = 'remote.example.com'\ntls = false",
            "Redis",
        ),
        ("[tenant_data]\nmax_open_targets = 0", "max_open_targets"),
    ] {
        // 单独构造覆盖表，避免在同一 TOML 内重复定义测试覆盖的表。
        let mut document: toml::Table = toml::from_str(PRODUCTION).unwrap();
        let replacement: toml::Table = toml::from_str(overlay).unwrap();
        for (key, value) in replacement {
            document.insert(key, value);
        }
        fixture.write(Environment::Prod, &toml::to_string(&document).unwrap());
        assert!(fixture.error(Environment::Prod).contains(field));
    }
    fixture.write(
        Environment::Prod,
        &PRODUCTION.replace("production-file-contract", "replace-with-unique-scope"),
    );
    assert!(fixture.error(Environment::Prod).contains("scope_id"));
}
