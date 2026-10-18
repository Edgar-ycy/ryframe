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
        Self {
            _directory: directory,
            backend,
        }
    }

    fn request(&self) -> Value {
        json!({
            "backend": self.backend,
            "environment": {"APP_ENV": "test"}
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
            br#"{"backend":"a","backend":"b","environment":{}}"#,
        )
        .0,
        2
    );
}

#[test]
fn missing_config_and_invalid_values_use_stable_redacted_errors() {
    let fixture = Fixture::new();
    let missing = json!({
        "backend": fixture.backend.join("private-secret-path"),
        "environment": {"APP_ENV": "test"}
    });
    let (code, response) = invoke("settings", &missing);
    assert_eq!(code, 1);
    assert_eq!(response["error"]["code"], "config_file_missing");
    let rendered = response.to_string();
    assert!(!rendered.contains("private-secret-path"));
    assert!(!rendered.contains("APP_ENV"));

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
    let state = fixture.backend.join("budget.json");
    let request = json!({
        "backend": fixture.backend,
        "environment": {
            "APP_ENV": "test",
            "APP_DATABASE_PASSWORD": "private-value"
        },
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
    let Some(binding) = super::check::python_tests::binding(executable).unwrap() else {
        // 链接或 UNC 布局保留原入口；不能伪造为本机受保护二进制。
        return;
    };
    let fixture = Fixture::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let output = Command::new("python")
        .args([
            "-B",
            "-c",
            r#"
import json, os, sys
from pathlib import Path
from unittest.mock import patch
sys.path.insert(0, str(Path(sys.argv[1]) / 'tools/python'))
from rate_limit_client import load_app_table, rate_limit_settings
backend = Path(sys.argv[2])
environment = {'APP_ENV': 'test'}
first = load_app_table(backend, environment)
assert first['rate_limit']['capacity'] == 100
(backend / 'config/app.toml').write_text(
    '[rate_limit]\ncapacity = 137\nwindow_secs = 61\n', encoding='utf-8')
second = load_app_table(backend, environment)
assert second['rate_limit']['capacity'] == 137
assert second['rate_limit']['window_secs'] == 61
with patch.dict(os.environ, {}, clear=True):
    third = rate_limit_settings(backend, {**environment, 'APP_RATE_LIMIT_CAPACITY': '149'})
assert third['capacity'] == 149
assert second['rate_limit']['capacity'] == 137
print(json.dumps({'calls': 3, 'capacities': [100, 137, 149]}))
"#,
        ])
        .args([root, &fixture.backend])
        .env("RYFRAME_RATE_LIMIT_TOOL_BINDING", binding)
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
fn node_and_python_callers_have_no_second_configuration_implementation() {
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
    let node = fs::read_to_string(root.join("tools/js/pacing.mjs")).unwrap();
    assert!(node.contains("__rate-limit"));
    assert!(!node.contains("pythonInvocation"));
    assert!(!node.contains("full_stack_rate_limit_config.py"));
    let python = fs::read_to_string(root.join("tools/python/rate_limit_client.py")).unwrap();
    assert!(python.contains("__rate-limit"));
    assert!(!python.contains("import tomllib"));
    for caller in [
        "ci_full_stack.py",
        "devex_clone_target_binding.py",
        "devex_clone_source_proof.py",
        "devex_identity_environment.py",
        "reference_fixture_browser.py",
        "reference_fixture_browser_security.py",
        "restore_source_binding.py",
    ] {
        let source = fs::read_to_string(root.join("tools/python").join(caller)).unwrap();
        assert!(
            source.contains("from rate_limit_client import"),
            "{caller} must use the thin Rust client"
        );
    }
}
