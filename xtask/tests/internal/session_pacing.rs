use std::{
    collections::BTreeMap,
    fs,
    future::Future,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{
    http_session::{RequestHook, RequestMetadata, SessionError},
    session_pacing::{
        PreparationHook, artifact,
        model::{NS_PER_MS, Pacer, Summary, Wait},
    },
};

#[path = "session_pacing/lifecycle.rs"]
mod lifecycle;
#[path = "session_pacing/model.rs"]
mod model;

fn authority() -> Value {
    json!({"format_version": 1, "global": {"capacity": 100, "window_ms": 60000},
        "user": {"enabled": false, "capacity": 500, "window_ms": 60000},
        "api": {"window_ms": 60000, "rules": {
            "POST /api/v1/auth/login": 5, "GET /api/v1/auth/csrf": 30,
            "POST /api/v1/auth/password-reset/complete": 3}},
        "login": {"capacity": 5, "window_ms": 60000}})
}

fn pacing() -> Value {
    json!({"authority_sha256": artifact::digest(&authority()).unwrap(),
        "request_interval_ms": 1000, "sample_prepare_wait_ms": 61250,
        "operation_intervals_ms": {"get_auth_csrf": 2250, "post_auth_login": 12250,
            "post_auth_password_reset_complete": 20250}})
}

fn identity() -> Value {
    json!({"tenant_id": "system", "username": "reader", "client_address": "198.18.10.1"})
}

fn catalog() -> BTreeMap<String, Value> {
    [
        ("get_auth_csrf", "GET", "/api/v1/auth/csrf"),
        ("post_auth_login", "POST", "/api/v1/auth/login"),
        ("post_auth_refresh", "POST", "/api/v1/auth/refresh"),
        ("post_auth_logout", "POST", "/api/v1/auth/logout"),
        (
            "post_auth_password_reset_complete",
            "POST",
            "/api/v1/auth/password-reset/complete",
        ),
        ("get_system_posts", "GET", "/api/v1/system/posts"),
    ]
    .into_iter()
    .map(|(name, method, path)| (name.into(), json!({"method": method, "path": path})))
    .collect()
}

fn metadata(name: &str) -> RequestMetadata {
    let catalog = catalog();
    RequestMetadata {
        operation: name.into(),
        method: catalog[name]["method"].as_str().unwrap().into(),
        route: catalog[name]["path"].as_str().unwrap().into(),
    }
}

fn pacer() -> Pacer {
    Pacer::new(&pacing(), &authority(), catalog(), &identity()).unwrap()
}

fn run<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(future)
}

fn category<T>(result: Result<T, SessionError>) -> &'static str {
    match result {
        Ok(_) => panic!("预期固定类别失败"),
        Err(error) => error.category,
    }
}

fn ms(value: u64) -> u64 {
    value * NS_PER_MS
}

struct Fixture {
    directory: TempDir,
    config: Value,
    artifacts: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let root = directory.path();
        fs::create_dir(root.join("config")).unwrap();
        fs::write(
            root.join("config/app.toml"),
            concat!(
                "[rate_limit]\nenabled = true\ncapacity = 100\nwindow_secs = 60\n",
                "enable_user_rate_limit = false\nuser_capacity = 500\nuser_window_secs = 60\n",
                "api_window_secs = 60\n[rate_limit.api_limits]\n",
                "\"POST /api/v1/auth/login\" = 5\n\"GET /api/v1/auth/csrf\" = 30\n",
                "\"POST /api/v1/auth/password-reset/complete\" = 3\n",
            ),
        )
        .unwrap();
        fs::write(
            root.join("config/app.test.toml"),
            "[auth]\njwt_secret = 'fixture-secret-not-for-receipts'\n",
        )
        .unwrap();
        fs::create_dir(root.join("openapi")).unwrap();
        let mut paths = json!({});
        for (name, operation) in catalog() {
            paths[operation["path"].as_str().unwrap()]
                [operation["method"].as_str().unwrap().to_lowercase()] =
                json!({"operationId": name});
        }
        fs::write(
            root.join("openapi/openapi.json"),
            serde_json::to_vec(&json!({
                "x-ryframe-api-prefix": {"value": "/api/v1"}, "paths": paths,
            }))
            .unwrap(),
        )
        .unwrap();
        let config = json!({"contract": {"pacing": pacing()}, "bindings": {
            "scope_id": "pacing-test", "pacing": {"python": root.join("unused-python.exe"),
                "login_budget_state": root.join("login-budget.json")}}});
        let artifacts = root.join("artifacts");
        Self {
            directory,
            config,
            artifacts,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }
    fn hook(&self) -> Result<PreparationHook, SessionError> {
        self.hook_for_env("test")
    }
    fn hook_for_env(&self, app_env: &str) -> Result<PreparationHook, SessionError> {
        run(PreparationHook::new(
            self.root(),
            &self.config,
            &identity(),
            &self.artifacts,
            app_env,
        ))
    }
    fn receipt(&self, name: &str) -> Value {
        serde_json::from_slice(&fs::read(self.artifacts.join(name)).unwrap()).unwrap()
    }
    fn budget(&self) -> PathBuf {
        self.config["bindings"]["pacing"]["login_budget_state"]
            .as_str()
            .unwrap()
            .into()
    }
}

#[test]
fn app_test_profile_overrides_common_values_without_reading_dev_profile() {
    let fixture = Fixture::new();
    let common = fixture.root().join("config/app.toml");
    let text = fs::read_to_string(&common).unwrap();
    fs::write(
        &common,
        text.replacen("enabled = true", "enabled = false", 1),
    )
    .unwrap();
    fs::write(
        fixture.root().join("config/app.test.toml"),
        "[rate_limit]\nenabled = true\n",
    )
    .unwrap();
    fs::write(
        fixture.root().join("config/app.dev.toml"),
        "fixture-invalid-dev-profile",
    )
    .unwrap();
    let mut hook = fixture.hook().unwrap();
    assert_eq!(
        fixture.receipt("pacing-authority.json")["authority"],
        authority()
    );
    run(hook.close()).unwrap();
}

#[test]
fn optional_config_directory_preserves_chinese_spaces_and_relative_or_absolute_paths() {
    for absolute in [false, true] {
        let mut fixture = Fixture::new();
        let relative = Path::new("配置 空间");
        let directory = fixture.root().join(relative);
        fs::create_dir(&directory).unwrap();
        for name in ["app.toml", "app.test.toml"] {
            fs::copy(
                fixture.root().join("config").join(name),
                directory.join(name),
            )
            .unwrap();
        }
        fs::write(
            fixture.root().join("config/app.test.toml"),
            "fixture-invalid-default-profile",
        )
        .unwrap();
        fixture.config["bindings"]["app_config_dir"] = if absolute {
            json!(directory)
        } else {
            json!(relative)
        };
        let mut hook = fixture.hook().unwrap();
        let receipt = fixture.receipt("pacing-authority.json");
        assert_eq!(receipt["authority"], authority());
        assert!(!receipt.to_string().contains("配置 空间"));
        run(hook.close()).unwrap();
    }
}

#[test]
fn optional_config_directory_rejects_nontext_empty_and_control_paths_with_fixed_receipts() {
    for value in [
        Value::Null,
        json!(true),
        json!(7),
        json!([]),
        json!({}),
        json!(""),
        json!("fixture-private-path\n"),
        json!("fixture-private-path\0"),
    ] {
        let mut fixture = Fixture::new();
        fixture.config["bindings"]["app_config_dir"] = value;
        assert_eq!(category(fixture.hook()), "pacing-authority");
        let receipt = fixture.receipt("pacing-authority.json");
        assert_eq!(receipt["failure"], json!("pacing-authority"));
        assert!(receipt.get("authority").is_none());
        assert!(receipt.get("actual_sha256").is_none());
        assert!(!receipt.to_string().contains("fixture-private-path"));
        assert!(!fixture.budget().exists());
    }
}

#[test]
fn explicit_app_env_selects_dev_and_rejects_all_other_profiles() {
    let fixture = Fixture::new();
    fs::write(fixture.root().join("config/app.dev.toml"), "").unwrap();
    fs::write(
        fixture.root().join("config/app.test.toml"),
        "fixture-invalid-unselected-profile",
    )
    .unwrap();
    let mut hook = fixture.hook_for_env("dev").unwrap();
    assert_eq!(
        fixture.receipt("pacing-authority.json")["authority"],
        authority()
    );
    run(hook.close()).unwrap();
    for app_env in ["", "prod", "Test", "test ", "fixture-private-profile\n"] {
        let fixture = Fixture::new();
        assert_eq!(category(fixture.hook_for_env(app_env)), "pacing-authority");
        let receipt = fixture.receipt("pacing-authority.json");
        assert_eq!(receipt["failure"], json!("pacing-authority"));
        assert!(receipt.get("authority").is_none());
        assert!(!receipt.to_string().contains("fixture-private-profile"));
        assert!(!fixture.budget().exists());
    }
}

#[test]
fn selected_profile_is_required_and_dynamic_file_errors_never_enter_public_receipts() {
    for document in [
        None,
        Some("[rate_limit]\ncapacity = 'fixture-private-value'\n"),
    ] {
        let fixture = Fixture::new();
        let selected = fixture.root().join("config/app.test.toml");
        match document {
            None => fs::remove_file(&selected).unwrap(),
            Some(text) => fs::write(&selected, text).unwrap(),
        }
        assert_eq!(category(fixture.hook()), "pacing-authority");
        let receipt = fixture.receipt("pacing-authority.json");
        assert_eq!(receipt["failure"], json!("pacing-authority"));
        let text = receipt.to_string();
        for private in [
            "fixture-private-value",
            "app.test.toml",
            "rate_limit.capacity",
            fixture.root().to_str().unwrap(),
        ] {
            assert!(!text.contains(private));
        }
        assert!(!fixture.budget().exists());
    }
}

#[test]
fn legacy_environment_payloads_do_not_override_file_authority_or_leak_into_receipts() {
    let mut fixture = Fixture::new();
    let legacy = json!({"APP_ENV": "prod", "APP_CONFIG_DIR": "fixture-legacy-directory",
        "APP_RATE_LIMIT_ENABLED": "false", "APP_RATE_LIMIT_CAPACITY": "1",
        "APP_RATE_LIMIT_API_WINDOW_SECS": "1",
        "APP_DATABASE_PASSWORD": "fixture-legacy-secret"});
    // 只构造旧输入数据，不设置或读取进程环境。
    fixture.config["environment"] = legacy.clone();
    fixture.config["bindings"]["environment"] = legacy;
    let mut hook = fixture.hook().unwrap();
    let receipt = fixture.receipt("pacing-authority.json");
    assert_eq!(receipt["authority"], authority());
    assert_eq!(receipt["actual_sha256"], pacing()["authority_sha256"]);
    for private in [
        "APP_CONFIG_DIR",
        "fixture-legacy-directory",
        "fixture-legacy-secret",
    ] {
        assert!(!receipt.to_string().contains(private));
    }
    run(hook.close()).unwrap();
}

#[test]
fn pacing_sources_have_no_environment_read_or_mapping_entry() {
    for source in [
        include_str!("../../src/session_pacing.rs"),
        include_str!("../../src/session_pacing/model.rs"),
        include_str!("../../src/session_pacing/artifact.rs"),
    ] {
        for forbidden in [
            "std::env",
            "env::var",
            "getenv",
            "set_var",
            "password_env",
            "environment: &BTreeMap",
            "APP_CONFIG_DIR",
            "APP_RATE_LIMIT_",
        ] {
            assert!(!source.contains(forbidden));
        }
    }
    let source = include_str!("../../src/session_pacing.rs");
    assert!(source.contains("app_env: &str"));
    assert!(source.contains("crate::rate_limit::authority(root, config_dir, app_env)"));
}
