use std::{
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::rate_limit::execute;

struct Fixture {
    _directory: TempDir,
    backend: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let local = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask 必须位于后端工作区")
            .join(".local-tests/xtask-rate-limit");
        fs::create_dir_all(&local).expect("应准备工作区中的限流协议测试目录");
        let directory = tempfile::Builder::new()
            .prefix("protocol-")
            .tempdir_in(local)
            .expect("应创建限流协议临时目录");
        let backend = directory.path().to_path_buf();
        fs::create_dir(backend.join("config")).unwrap();
        fs::write(
            backend.join("config/app.toml"),
            r#"
[rate_limit]
capacity = 100
window_secs = 60
api_window_secs = 60
[rate_limit.api_limits]
"POST /api/v1/auth/login" = 5
"#,
        )
        .unwrap();
        fs::write(backend.join("config/app.test.toml"), "# 所选环境\n").unwrap();
        Self {
            _directory: directory,
            backend,
        }
    }

    fn request(&self) -> Value {
        json!({
            "backend": self.backend,
            "app_env": "test"
        })
    }
}

fn invoke(operation: &str, request: &Value) -> (i32, Value) {
    let (code, output) = execute(
        &[OsString::from(operation)],
        &serde_json::to_vec(request).unwrap(),
    );
    (code, serde_json::from_str(&output).unwrap())
}

#[test]
fn protocol_exposes_only_the_result_and_rejects_unknown_or_private_fields() {
    let fixture = Fixture::new();
    let (code, response) = invoke("authority", &fixture.request());
    assert_eq!(code, 0);
    assert_eq!(response["ok"], true);
    assert_eq!(response["result"]["format_version"], 1);
    assert_eq!(response["result"]["login"]["capacity"], 5);

    let mut request = fixture.request();
    request["extra"] = json!("private-value");
    let (code, response) = invoke("authority", &request);
    assert_eq!(code, 2);
    assert_eq!(response["error"]["code"], "invalid_request");
    assert!(!response.to_string().contains("private-value"));
    assert_eq!(
        execute(
            &[OsString::from("authority"), OsString::from("extra")],
            b"{}"
        )
        .0,
        2
    );
    assert_eq!(
        execute(
            &[OsString::from("authority")],
            br#"{"backend":"a","backend":"b","app_env":"test"}"#,
        )
        .0,
        2
    );
}

#[test]
fn missing_config_and_invalid_values_use_stable_redacted_errors() {
    let fixture = Fixture::new();
    let missing = json!({
        "backend": fixture.backend.join("missing-config"),
        "app_env": "test"
    });
    let (code, response) = invoke("settings", &missing);
    assert_eq!(code, 1);
    assert_eq!(response["error"]["code"], "config_document_invalid");
    let rendered = response.to_string();
    assert!(rendered.contains("missing-config"));
    assert!(rendered.contains("app.test.toml"));
    assert!(rendered.contains("请"));

    fs::write(
        fixture.backend.join("config/app.toml"),
        "[rate_limit]\ncapacity = 0\n",
    )
    .unwrap();
    let (code, response) = invoke("authority", &fixture.request());
    assert_eq!(code, 1);
    assert_eq!(response["error"]["code"], "rate_limit_integer_invalid");
    let oversized = vec![b'x'; 1024 * 1024 + 1];
    let (code, response) = execute(&[OsString::from("authority")], &oversized);
    assert_eq!(code, 2);
    assert_eq!(
        serde_json::from_str::<Value>(&response).unwrap()["error"]["code"],
        "invalid_request"
    );
}

#[test]
fn login_budget_protocol_returns_only_the_three_runtime_values() {
    let fixture = Fixture::new();
    let file = fixture.backend.join("config/app.toml");
    fs::write(
        &file,
        format!(
            "[auth]\njwt_secret='private-value'\n{}",
            fs::read_to_string(&file).unwrap()
        ),
    )
    .unwrap();
    let state = fixture.backend.join("budget.json");
    let request = json!({
        "backend": fixture.backend,
        "app_env": "test",
        "state_path": state
    });
    let (code, response) = invoke("login-budget", &request);
    assert_eq!(code, 0);
    assert_eq!(
        response["result"],
        json!({
            "RYFRAME_E2E_LOGIN_BUDGET_STATE": state,
            "RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY": "5",
            "RYFRAME_E2E_LOGIN_RATE_LIMIT_WINDOW_SECS": "60"
        })
    );
    assert!(!response.to_string().contains("private-value"));
}

#[test]
fn hidden_binary_is_absent_from_help_and_reads_the_request_from_stdin() {
    let help = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("__rate-limit"));

    let fixture = Fixture::new();
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["__rate-limit", "authority"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&fixture.request()).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["result"]["login"]["capacity"], 5);

    let failed = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("__rate-limit")
        .output()
        .unwrap();
    assert_eq!(failed.status.code(), Some(2));
    assert_eq!(
        serde_json::from_slice::<Value>(&failed.stdout).unwrap()["error"]["code"],
        "invalid_request"
    );
    assert!(failed.stderr.is_empty());
}

#[test]
fn python_bound_tool_reloads_configuration_without_starting_cargo() {
    let executable = Path::new(env!("CARGO_BIN_EXE_xtask"));
    let Some(binding) = super::check::python_suite::binding(executable).unwrap() else {
        // 链接或 UNC 布局保留原入口；不能伪造为本机受保护二进制。
        return;
    };
    let fixture = Fixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let program = super::check::consumer_context::python::selected_program(root).unwrap();
    let output = Command::new(program)
        .args([
            "-X",
            "utf8",
            "-B",
            "-c",
            r#"
import json, os, sys
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0, str(Path(sys.argv[1]) / 'tools/python'))
from rate_limit_tool import install_binding
install_binding(sys.argv[3])
from rate_limit_client import load_app_table
backend = Path(sys.argv[2])
environment = {'APP_ENV': 'test'}
first = load_app_table(backend, environment, config_dir=backend / "config")
assert first['rate_limit']['capacity'] == 100
(backend / 'config/app.toml').write_text(
    '[rate_limit]\ncapacity = 137\nwindow_secs = 61\n', encoding='utf-8')
second = load_app_table(backend, environment, config_dir=backend / "config")
assert second['rate_limit']['capacity'] == 137
assert second['rate_limit']['window_secs'] == 61
(backend / 'config/app.toml').write_text(
    '[rate_limit]\ncapacity = 149\nwindow_secs = 61\n', encoding='utf-8')
with patch.dict(os.environ, {}, clear=True):
    third = load_app_table(backend, environment, config_dir=backend / 'config')
assert third['rate_limit']['capacity'] == 149
assert second['rate_limit']['capacity'] == 137
print(json.dumps({'calls': 3, 'capacities': [100, 137, 149]}))
"#,
        ])
        .args([root, &fixture.backend])
        .arg(binding)
        .env("RYFRAME_RATE_LIMIT_TOOL_BINDING", "invalid-legacy-binding")
        .env("CARGO", "ryframe-cargo-must-not-be-started")
        .env("APP_RATE_LIMIT_CAPACITY", "991")
        .env("PYTHONUTF8", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"calls": 3, "capacities": [100, 137, 149]})
    );
}

#[test]
fn native_and_python_callers_share_the_rust_configuration_authority() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask 必须位于后端工作区");
    assert!(
        !root
            .join("tools/python/full_stack_rate_limit_config.py")
            .exists()
    );
    assert!(
        !root
            .join("tools/python/test_full_stack_rate_limit_config.py")
            .exists()
    );
    let native = fs::read_to_string(root.join("xtask/src/session_pacing.rs")).unwrap();
    assert!(native.contains("crate::rate_limit::authority(root, config_dir, app_env)"));
    assert!(!native.contains("std::process"));
    assert!(!native.contains("toml::"));
    let python = fs::read_to_string(root.join("tools/python/rate_limit_client.py")).unwrap();
    assert!(python.contains("__rate-limit"));
    assert!(!python.contains("import tomllib"));
    for caller in [
        "devex_clone_target_binding.py",
        "devex_clone_source_proof.py",
        "fixture_browser_dependencies.py",
    ] {
        let source = fs::read_to_string(root.join("tools/python").join(caller)).unwrap();
        assert!(
            source.contains("from rate_limit_client import"),
            "{caller} must use the thin Rust client"
        );
    }
    assert!(
        !root
            .join("tools/python/reference_fixture_browser_security.py")
            .exists(),
        "浏览器安全协调必须由 Rust fixture runtime 提供"
    );
}

#[test]
fn file_context_rejects_old_overrides_unknown_fields_and_invalid_selectors() {
    let fixture = Fixture::new();
    for field in [
        "environment",
        "APP_CONFIG_DIR",
        "APP_RATE_LIMIT_ENABLED",
        "APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT",
        "APP_RATE_LIMIT_CAPACITY",
        "APP_RATE_LIMIT_WINDOW_SECS",
        "APP_RATE_LIMIT_USER_CAPACITY",
        "APP_RATE_LIMIT_USER_WINDOW_SECS",
        "APP_RATE_LIMIT_API_WINDOW_SECS",
    ] {
        let mut request = fixture.request();
        request[field] = json!({"APP_ENV": "test", "value": "private-value"});
        for operation in ["load-table", "settings", "authority", "login-budget"] {
            if operation == "login-budget" {
                request["state_path"] = json!(fixture.backend.join("budget.json"));
            }
            let (code, response) = invoke(operation, &request);
            assert_eq!(code, 2);
            assert_eq!(response["error"]["code"], "invalid_request");
            assert!(!response.to_string().contains("private-value"));
        }
    }
    for value in [json!(false), json!(1)] {
        let mut request = fixture.request();
        request["config_dir"] = value;
        assert_eq!(invoke("load-table", &request).0, 2);
    }
    for value in ["", "bad\npath", "nul\0path"] {
        let mut request = fixture.request();
        request["config_dir"] = json!(value);
        assert_eq!(
            invoke("load-table", &request).1["error"]["code"],
            "config_directory_invalid"
        );
    }
    for app_env in ["", "prod", " test", "TEST"] {
        let mut request = fixture.request();
        request["app_env"] = json!(app_env);
        assert_eq!(
            invoke("load-table", &request).1["error"]["code"],
            "config_environment_invalid"
        );
    }
    let mut request = fixture.request();
    request.as_object_mut().unwrap().remove("app_env");
    assert_eq!(invoke("load-table", &request).0, 2);
}

#[test]
fn omitted_and_null_configuration_directories_use_the_same_default() {
    let fixture = Fixture::new();
    for operation in ["load-table", "settings", "authority", "login-budget"] {
        let mut omitted = fixture.request();
        if operation == "login-budget" {
            omitted["state_path"] = json!(fixture.backend.join("budget.json"));
        }
        let expected = invoke(operation, &omitted);
        assert_eq!(expected.0, 0);
        let mut nullable = omitted;
        nullable["config_dir"] = Value::Null;
        assert_eq!(invoke(operation, &nullable), expected);
    }
}

#[test]
fn explicit_file_context_selects_relative_or_absolute_directories_and_exact_environment() {
    let fixture = Fixture::new();
    let directory = fixture.backend.join("role-config");
    fs::rename(fixture.backend.join("config"), &directory).unwrap();
    fs::write(
        directory.join("app.dev.toml"),
        "[rate_limit]\ncapacity=137\n",
    )
    .unwrap();
    fs::write(
        directory.join("app.test.toml"),
        "[rate_limit]\ncapacity=149\n",
    )
    .unwrap();
    for config_dir in [Path::new("role-config"), directory.as_path()] {
        for (app_env, capacity) in [("dev", 137), ("test", 149)] {
            let mut request = fixture.request();
            request["config_dir"] = json!(config_dir);
            request["app_env"] = json!(app_env);
            for operation in ["load-table", "settings", "authority"] {
                let (code, response) = invoke(operation, &request);
                assert_eq!(code, 0, "{response}");
                let actual = match operation {
                    "load-table" => &response["result"]["rate_limit"]["capacity"],
                    "settings" => &response["result"]["capacity"],
                    _ => &response["result"]["global"]["capacity"],
                };
                assert_eq!(actual, &json!(capacity));
            }
            request["state_path"] = json!(fixture.backend.join("budget.json"));
            let (code, response) = invoke("login-budget", &request);
            assert_eq!(code, 0, "{response}");
            assert_eq!(
                response["result"]["RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY"],
                "5"
            );
            assert_eq!(response["result"].as_object().unwrap().len(), 3);
        }
    }
    assert_eq!(
        invoke("load-table", &fixture.request()).1["error"]["code"],
        "config_document_invalid"
    );
}
