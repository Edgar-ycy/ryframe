use std::{fs, path::Path, process::Command};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::check::{PYTHON_TEST_ARGS, consumer_context::python, python_suite};

#[test]
fn suite_keeps_each_binding_argument_and_every_unittest_selection() {
    let binding = r#"{"path":"D:/中文 工具/xtask.exe","sha256":"fixed"}"#;
    let arguments = python_suite::arguments(Some(binding), false);
    assert_eq!(
        arguments[..4],
        ["-X", "utf8", "-B", "tools/python/run_checks.py"]
    );
    assert_eq!(
        arguments[4..7],
        ["--rate-limit-tool-binding", binding, "--"]
    );
    assert_eq!(arguments[7..], *PYTHON_TEST_ARGS);
}

#[test]
fn suite_without_a_protected_binding_explicitly_uses_the_unbound_bootstrap() {
    let arguments = python_suite::arguments(None, false);
    assert_eq!(arguments[4], "--");
    assert_eq!(arguments[5..], *PYTHON_TEST_ARGS);
    assert!(
        !arguments
            .iter()
            .any(|argument| argument == "--rate-limit-tool-binding")
    );
}

struct BootstrapFixture {
    directory: TempDir,
}

impl BootstrapFixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let parent = root.join(".local-tests/python-bootstrap");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::Builder::new()
            .prefix("checks-")
            .tempdir_in(parent)
            .unwrap();
        fs::create_dir(directory.path().join("config")).unwrap();
        fs::write(
            directory.path().join("config/app.test.toml"),
            "[rate_limit]\ncapacity=173\n",
        )
        .unwrap();
        Self { directory }
    }

    fn run(&self, source: &str, bound: bool) -> std::process::Output {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let source = source.replace(
            "{backend}",
            &serde_json::to_string(self.directory.path()).unwrap(),
        );
        fs::write(self.directory.path().join("test_bound_probe.py"), source).unwrap();
        let binding = if bound {
            Some(
                crate::rate_limit::tool_binding(Path::new(env!("CARGO_BIN_EXE_xtask")))
                    .unwrap()
                    .unwrap(),
            )
        } else {
            None
        };
        let mut arguments = python_suite::arguments(binding.as_deref(), true);
        let pattern = arguments
            .iter()
            .position(|argument| argument == "-p")
            .unwrap()
            + 1;
        arguments[pattern] = "test_bound_probe.py".into();
        let directory = arguments
            .iter()
            .position(|argument| argument == "-s")
            .unwrap()
            + 1;
        arguments[directory] = self.directory.path().to_str().unwrap().into();
        Command::new(python::selected_program(root).unwrap())
            .args(arguments)
            .current_dir(root)
            .env("CARGO", "ryframe-cargo-must-not-be-started")
            .env("RYFRAME_RATE_LIMIT_TOOL_BINDING", "invalid-legacy-binding")
            .output()
            .unwrap()
    }
}

#[test]
fn bootstrap_passes_the_real_native_binding_without_a_project_environment_entry() {
    let fixture = BootstrapFixture::new();
    let output = fixture.run(
        r#"import json, unittest
from pathlib import Path
from rate_limit_client import load_app_table
from rate_limit_tool import tool_binding
class Probe(unittest.TestCase):
    def test_binding_and_actual_native_document(self):
        self.assertIsNotNone(tool_binding())
        self.assertEqual(load_app_table(Path({backend}), {'APP_ENV':'test'})['rate_limit']['capacity'], 173)
        print(json.dumps({'native_called': True}))
"#,
        true,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "test_binding_and_actual_native_document (test_bound_probe.Probe.test_binding_and_actual_native_document)"
    ));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"native_called":true})
    );
}

#[test]
fn bootstrap_preserves_a_real_unittest_failure_exit_status() {
    let fixture = BootstrapFixture::new();
    let output = fixture.run(
        "import unittest\nclass Probe(unittest.TestCase):\n    def test_expected_failure(self):\n        self.fail('bootstrap-must-propagate-test-failure')\n",
        true,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("bootstrap-must-propagate-test-failure")
    );
}

#[test]
fn bootstrap_without_an_explicit_binding_ignores_the_retired_environment_entry() {
    let fixture = BootstrapFixture::new();
    let output = fixture.run(
        "import unittest\nfrom rate_limit_tool import tool_binding\nclass Probe(unittest.TestCase):\n    def test_unbound(self):\n        self.assertIsNone(tool_binding())\n",
        false,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn target_fixture_installs_the_explicit_tool_and_preserves_script_argument_positions() {
    let output = crate::target_configuration_fixture::python_command(
        "import json, sys\nfrom rate_limit_tool import tool_binding\nprint(json.dumps({'argument':sys.argv[1], 'bound':tool_binding() is not None}))\n",
    )
    .arg("中文 路径参数")
    .env("RYFRAME_RATE_LIMIT_TOOL_BINDING", "invalid-legacy-binding")
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"argument":"中文 路径参数", "bound":true})
    );
}
