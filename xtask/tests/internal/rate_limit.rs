use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::json;
use tempfile::TempDir;

use super::rate_limit::{
    DEFAULT_API_WINDOW_SECS, DEFAULT_LOGIN_CAPACITY, LOGIN_RULE, authority, load_app_table,
    login_budget_environment, login_rate_limit, settings,
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

    fn environment(&self) -> BTreeMap<String, String> {
        BTreeMap::from([("APP_ENV".to_owned(), "test".to_owned())])
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
fn toml_tables_merge_recursively_and_environment_overrides_are_typed() {
    let fixture = Fixture::new(BASE);
    fixture.write_override(
        "test",
        r#"
[app]
port = 8080

[rate_limit]
api_window_secs = 90

[rate_limit.api_limits]
"POST /api/v1/auth/login" = 7
"#,
    );
    let mut environment = fixture.environment();
    environment.insert("APP_RATE_LIMIT_API_WINDOW_SECS".into(), "+30".into());
    environment.insert(
        "APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT".into(),
        "true".into(),
    );
    environment.insert("APP_RATE_LIMIT_USER_CAPACITY".into(), "12".into());
    let table = load_app_table(&fixture.backend, &environment).unwrap();
    assert_eq!(table["app"]["host"].as_str(), Some("127.0.0.1"));
    assert_eq!(table["app"]["port"].as_integer(), Some(8080));
    assert_eq!(
        table["rate_limit"]["api_limits"]["POST /api/v1/auth/login"].as_integer(),
        Some(7)
    );
    assert_eq!(
        table["rate_limit"]["api_limits"]["GET /api/v1/auth/csrf"].as_integer(),
        Some(30)
    );

    let settings = settings(&fixture.backend, &environment).unwrap();
    assert_eq!(settings["enable_user_rate_limit"].as_bool(), Some(true));
    assert_eq!(settings["user_capacity"].as_integer(), Some(12));
    assert_eq!(settings["api_window_secs"].as_integer(), Some(30));
}

#[test]
fn full_authority_matches_the_existing_shape_without_environment_secrets() {
    let fixture = Fixture::new(BASE);
    let environment = BTreeMap::from([
        ("APP_ENV".to_owned(), "test".to_owned()),
        (
            "APP_DATABASE_PASSWORD".to_owned(),
            "private-value".to_owned(),
        ),
        ("APP_RATE_LIMIT_CAPACITY".to_owned(), "80".to_owned()),
        ("APP_RATE_LIMIT_WINDOW_SECS".to_owned(), "45".to_owned()),
        (
            "APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT".to_owned(),
            "true".to_owned(),
        ),
        ("APP_RATE_LIMIT_USER_CAPACITY".to_owned(), "12".to_owned()),
        (
            "APP_RATE_LIMIT_USER_WINDOW_SECS".to_owned(),
            "30".to_owned(),
        ),
        ("APP_RATE_LIMIT_API_WINDOW_SECS".to_owned(), "90".to_owned()),
    ]);
    let value = serde_json::to_value(authority(&fixture.backend, &environment).unwrap()).unwrap();
    assert_eq!(
        value,
        json!({
            "format_version": 1,
            "global": {"capacity": 80, "window_ms": 45000},
            "user": {"enabled": true, "capacity": 12, "window_ms": 30000},
            "api": {"rules": {
                "GET /api/v1/auth/csrf": 30,
                "POST /api/v1/auth/login": 5
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
    let environment = BTreeMap::from([("APP_ENV".to_owned(), "test".to_owned())]);
    let value = authority(root, &environment).unwrap();
    let value = serde_json::to_value(value).unwrap();
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
    for (variable, value) in [
        ("APP_RATE_LIMIT_ENABLED", "false"),
        ("APP_RATE_LIMIT_ENABLED", "TRUE"),
        ("APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT", "yes"),
        ("APP_RATE_LIMIT_CAPACITY", "0"),
        ("APP_RATE_LIMIT_CAPACITY", "-1"),
        ("APP_RATE_LIMIT_CAPACITY", " 60"),
        ("APP_RATE_LIMIT_CAPACITY", "true"),
        ("APP_RATE_LIMIT_WINDOW_SECS", "86401"),
        ("APP_RATE_LIMIT_API_WINDOW_SECS", "x"),
    ] {
        let fixture = Fixture::new(BASE);
        let mut environment = fixture.environment();
        environment.insert(variable.into(), value.into());
        assert!(
            settings(&fixture.backend, &environment).is_err(),
            "{variable}={value}"
        );
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
        let environment = fixture.environment();
        assert!(
            settings(&fixture.backend, &environment).is_err(),
            "{source}"
        );
    }
    let fixture = Fixture::new(BASE);
    for changes in [("APP_ENV", ""), ("APP_ENV", "prod"), ("APP_CONFIG_DIR", "")] {
        let mut environment = fixture.environment();
        environment.insert(changes.0.into(), changes.1.into());
        assert!(load_app_table(&fixture.backend, &environment).is_err());
    }
}

#[test]
fn login_limit_and_budget_environment_keep_the_explicit_boundary() {
    let fixture = Fixture::new(BASE);
    let environment = fixture.environment();
    assert_eq!(
        login_rate_limit(&fixture.backend, &environment)
            .unwrap()
            .capacity,
        5
    );
    let values = login_budget_environment(
        &fixture.backend,
        &environment,
        fixture.backend.join("budget.json").to_str().unwrap(),
    )
    .unwrap();
    assert_eq!(values.len(), 3);
    assert_eq!(values["RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY"], "5");
    for path in ["relative.json", "line\nbreak.json", "nul\0byte.json"] {
        assert!(login_budget_environment(&fixture.backend, &environment, path).is_err());
    }
    let over = Fixture::new(
        "[rate_limit]\napi_window_secs = 60\n[rate_limit.api_limits]\n\
         \"POST /api/v1/auth/login\" = 10001\n",
    );
    assert!(login_rate_limit(&over.backend, &over.environment()).is_err());
}
