use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::json;
use tempfile::TempDir;

use super::rate_limit::{
    DEFAULT_API_WINDOW_SECS, DEFAULT_LOGIN_CAPACITY, LOGIN_RULE, authority, load_app_table,
    settings,
};

struct Fixture {
    _directory: TempDir,
    backend: PathBuf,
}

impl Fixture {
    fn new(base: &str) -> Self {
        let directory = TempDir::new().expect("应创建限流配置临时目录");
        let backend = directory.path().to_path_buf();
        fs::create_dir(backend.join("config")).expect("应创建配置目录");
        fs::write(backend.join("config/app.toml"), base).expect("应写入基础配置");
        fs::write(backend.join("config/app.test.toml"), "# 所选环境\n").unwrap();
        Self {
            _directory: directory,
            backend,
        }
    }

    fn write_override(&self, environment: &str, source: &str) {
        fs::write(
            self.backend
                .join("config")
                .join(format!("app.{environment}.toml")),
            source,
        )
        .expect("应写入环境配置");
    }
}

const BASE: &str = r#"
[app]
host = "127.0.0.1"

[rate_limit]
enabled = true
capacity = 10
window_secs = 60
enable_user_rate_limit = false
user_capacity = 20
user_window_secs = 50
api_window_secs = 40

[rate_limit.api_limits]
"POST /api/v1/auth/login" = 5
"GET /api/v1/auth/csrf" = 30
"#;

#[test]
fn toml_tables_merge_recursively_and_environment_file_values_are_typed() {
    let fixture = Fixture::new(BASE);
    fixture.write_override(
        "test",
        r#"
[app]
port = 8080
[rate_limit]
api_window_secs = 30
enable_user_rate_limit = true
user_capacity = 12
[rate_limit.api_limits]
"POST /api/v1/auth/login" = 7
"#,
    );
    let table = load_app_table(&fixture.backend, None, "test").unwrap();
    assert_eq!(table["app"]["host"].as_str(), Some("127.0.0.1"));
    assert_eq!(table["app"]["port"].as_integer(), Some(8080));
    assert_eq!(
        table["rate_limit"]["api_limits"][LOGIN_RULE].as_integer(),
        Some(7)
    );
    assert_eq!(
        table["rate_limit"]["api_limits"]["GET /api/v1/auth/csrf"].as_integer(),
        Some(30)
    );
    let settings = settings(&fixture.backend, None, "test").unwrap();
    assert_eq!(settings["enable_user_rate_limit"].as_bool(), Some(true));
    assert_eq!(settings["user_capacity"].as_integer(), Some(12));
    assert_eq!(settings["api_window_secs"].as_integer(), Some(30));
}

#[test]
fn full_authority_matches_the_existing_shape_without_file_secrets() {
    let fixture = Fixture::new(&format!("[auth]\njwt_secret='private-value'\n{BASE}"));
    fixture.write_override("test", "[rate_limit]\ncapacity=80\nwindow_secs=45\nenable_user_rate_limit=true\nuser_capacity=12\nuser_window_secs=30\napi_window_secs=90\n");
    let value = serde_json::to_value(authority(&fixture.backend, None, "test").unwrap()).unwrap();
    assert_eq!(
        value,
        json!({
            "format_version": 1,
            "global": {"capacity": 80, "window_ms": 45000},
            "user": {"enabled": true, "capacity": 12, "window_ms": 30000},
            "api": {"rules": {
                "GET /api/v1/auth/csrf": 30, "POST /api/v1/auth/login": 5
            }, "window_ms": 90000},
            "login": {"capacity": 5, "window_ms": 90000}
        })
    );
    assert!(!value.to_string().contains("private-value"));
}

#[test]
fn defaults_rules_and_product_config_cannot_drift_silently() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask 必须位于后端工作区");
    let fixture = Fixture::new(&fs::read_to_string(root.join("config/app.toml")).unwrap());
    let value = serde_json::to_value(authority(&fixture.backend, None, "test").unwrap()).unwrap();
    assert_eq!(
        value["global"],
        json!({"capacity": 100, "window_ms": 60000})
    );
    assert_eq!(
        value["user"],
        json!({"enabled": false, "capacity": 500, "window_ms": 60000})
    );
    assert_eq!(value["api"]["rules"][LOGIN_RULE], 5);
    assert_eq!(value["api"]["rules"]["GET /api/v1/auth/csrf"], 30);
    assert_eq!(
        value["api"]["rules"]["POST /api/v1/auth/password-reset/complete"],
        3
    );
    assert_eq!(value["api"]["window_ms"], 60000);
    assert_eq!(DEFAULT_API_WINDOW_SECS, 60);
    assert_eq!(DEFAULT_LOGIN_CAPACITY, 5);
    let product = ryframe_config::RateLimitConfig::default();
    assert_eq!(product.api_window_secs, DEFAULT_API_WINDOW_SECS);
    assert_eq!(product.capacity, 100);
}

#[test]
fn booleans_integers_windows_and_api_limits_fail_closed() {
    for (field, value) in [
        ("enabled", "false"),
        ("enabled", "'TRUE'"),
        ("enable_user_rate_limit", "'yes'"),
        ("capacity", "0"),
        ("capacity", "-1"),
        ("capacity", "' 60'"),
        ("capacity", "true"),
        ("window_secs", "86401"),
        ("api_window_secs", "'x'"),
    ] {
        let fixture = Fixture::new(BASE);
        fixture.write_override("test", &format!("[rate_limit]\n{field}={value}\n"));
        assert!(settings(&fixture.backend, None, "test").is_err(), "{field}");
    }
    for source in [
        "[rate_limit]\ncapacity = 0",
        "[rate_limit]\ncapacity = \"10\"",
        "[rate_limit]\nwindow_secs = true",
        "[rate_limit]\nenabled = false",
        "[rate_limit]\napi_limits = \"bad\"",
        "[rate_limit.api_limits]\n\"POST /api/v1/auth/login\" = 0",
        "[rate_limit.api_limits]\n\" \" = 1",
        "[rate_limit.api_limits]\n\"POST /api/v1/auth/login\" = 4294967296",
    ] {
        let fixture = Fixture::new(source);
        assert!(
            settings(&fixture.backend, None, "test").is_err(),
            "{source}"
        );
    }
    let fixture = Fixture::new(BASE);
    for app_env in ["", "prod", " test", "TEST", "production"] {
        assert_eq!(
            load_app_table(&fixture.backend, None, app_env)
                .unwrap_err()
                .code(),
            "config_environment_invalid"
        );
    }
    for directory in ["", "bad\npath", "bad\tpath", "nul\0path"] {
        assert_eq!(
            load_app_table(&fixture.backend, Some(Path::new(directory)), "test")
                .unwrap_err()
                .code(),
            "config_directory_invalid"
        );
    }
}

#[test]
fn selected_only_directories_missing_selected_and_bad_common_are_not_silent() {
    let fixture = Fixture::new(BASE);
    fs::remove_file(fixture.backend.join("config/app.toml")).unwrap();
    fixture.write_override("test", BASE);
    assert_eq!(
        settings(&fixture.backend, Some(Path::new("config")), "test").unwrap()["capacity"]
            .as_integer(),
        Some(10)
    );
    assert!(
        load_app_table(
            &fixture.backend,
            Some(&fixture.backend.join("config")),
            "test"
        )
        .is_ok()
    );
    fs::remove_file(fixture.backend.join("config/app.test.toml")).unwrap();
    let missing = load_app_table(&fixture.backend, None, "test").unwrap_err();
    assert!(missing.message().contains("app.test.toml"));
    fixture.write_override("test", BASE);
    fs::write(
        fixture.backend.join("config/app.toml"),
        "password='private-value'\n[broken",
    )
    .unwrap();
    let invalid = load_app_table(&fixture.backend, None, "test").unwrap_err();
    assert!(invalid.message().contains("app.toml"));
    assert!(invalid.message().contains("TOML"));
    assert!(!invalid.message().contains("private-value"));
}

#[test]
fn capacity_and_window_boundaries_keep_the_original_safe_ranges() {
    for (field, maximum) in [
        ("capacity", i64::from(u32::MAX)),
        ("user_capacity", i64::from(u32::MAX)),
        ("window_secs", 86_400),
        ("user_window_secs", 86_400),
        ("api_window_secs", 86_400),
    ] {
        let fixture = Fixture::new(BASE);
        for accepted in [1, maximum] {
            fixture.write_override("test", &format!("[rate_limit]\n{field}={accepted}\n"));
            assert_eq!(
                settings(&fixture.backend, None, "test").unwrap()[field].as_integer(),
                Some(accepted)
            );
        }
        for rejected in [0, -1, maximum + 1] {
            fixture.write_override("test", &format!("[rate_limit]\n{field}={rejected}\n"));
            let error = settings(&fixture.backend, None, "test").unwrap_err();
            assert_eq!(error.code(), "rate_limit_integer_invalid");
            assert!(error.message().contains(&format!("rate_limit.{field}")));
            assert!(error.message().contains("app.test.toml"));
            assert!(error.message().contains("请"));
        }
    }
}
