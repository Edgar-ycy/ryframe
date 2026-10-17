use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};

use super::{
    recovery::clone::native::{bridge, digest, maintenance, source},
    workspace,
};

static NEXT: AtomicUsize = AtomicUsize::new(1);

struct Fixture {
    root: PathBuf,
    local: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = workspace::root_dir().join(".local-tests").join(format!(
            "clone-bridge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".local-tests")).unwrap();
        write(&root.join(".gitignore"), ".local-tests/\ntarget/\n");
        write(&root.join("Cargo.toml"), "[workspace]\n");
        write(&root.join("Cargo.lock"), "version = 4\n");
        write(&root.join("product/input.txt"), "product\n");
        write(
            &root.join("crates/ryframe/Cargo.toml"),
            "[package]\nname='ryframe'\n",
        );
        write(&root.join("crates/ryframe/src/main.rs"), "fn main() {}\n");
        write(
            &root.join("crates/ryframe-api/src/lib.rs"),
            "pub fn api() {}\n",
        );
        write(&root.join("tools/python/check.py"), "pass\n");
        write(&root.join("docs/data.md"), "# data\n");
        git(&root, &["init", "--quiet"]);
        git(&root, &["config", "user.name", "clone-bridge-test"]);
        git(&root, &["config", "user.email", "test@example.invalid"]);
        git(&root, &["config", "core.autocrlf", "false"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "fixture"]);
        Self {
            local: root.join(".local-tests"),
            root,
        }
    }

    fn restore_inputs(&self) -> (PathBuf, PathBuf) {
        let inventory = source::capture(&self.root).unwrap();
        let local = self.local.join("restore");
        fs::create_dir_all(&local).unwrap();
        let mut artifacts = serde_json::Map::new();
        for (role, feature, name) in [
            ("api", "bin-api", "ryframe"),
            ("worker", "bin-worker", "ryframe-worker"),
        ] {
            let executable = local.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            fs::write(&executable, format!("fixed {role}")).unwrap();
            let file = digest::file(&executable).unwrap();
            artifacts.insert(
                role.into(),
                json!({
                    "executable": executable,
                    "command": [
                        "cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                        "--features", feature, "--bin", name, "--message-format=json"
                    ],
                    "bytes": file.bytes,
                    "sha256": file.sha256,
                }),
            );
        }
        let build = json!({
            "format_version": 2,
            "kind": "restore-backend-build",
            "sources": source::backend_domains(&inventory).unwrap(),
            "build": {
                "commands": {
                    "api": [
                        "cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                        "--features", "bin-api", "--bin", "ryframe", "--message-format=json"
                    ],
                    "worker": [
                        "cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                        "--features", "bin-worker", "--bin", "ryframe-worker", "--message-format=json"
                    ],
                },
                "profile": "dev",
                "target": "x86_64-test",
                "jobs": "cargo-default",
                "toolchain": {"cargo": "cargo fixture", "rustc": "rustc fixture"},
                "environment": {"variables": [], "sha256": "a".repeat(64)},
            },
            "artifacts": artifacts,
        });
        let build_path = local.join("build.json");
        let inventory_path = local.join("inventory.json");
        write_json(&build_path, &build);
        write_json(&inventory_path, &source::inventory_value(&inventory));
        (build_path, inventory_path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.root
                .starts_with(workspace::root_dir().join(".local-tests"))
        );
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct FakeRunner {
    calls: Cell<usize>,
    built: RefCell<Vec<String>>,
    fail_name: Option<String>,
    diagnostic: Option<String>,
}

impl maintenance::Runner for FakeRunner {
    fn version(
        &self,
        _root: &Path,
        executable: &str,
        _arguments: &[&str],
    ) -> Result<String, Box<dyn std::error::Error>> {
        Ok(format!("{executable} fixture"))
    }

    fn build(
        &self,
        root: &Path,
        arguments: &[String],
        stdout: &Path,
        stderr: &Path,
        commit: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.calls.set(self.calls.get() + 1);
        let name = arguments
            .iter()
            .position(|value| value == "--bin")
            .and_then(|index| arguments.get(index + 1))
            .unwrap();
        self.built.borrow_mut().push(name.clone());
        let executable = root
            .join("target/ci/backend/debug")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, format!("fixture {name}")).unwrap();
        fs::write(
            stdout,
            format!(
                "{}\n",
                serde_json::to_string(&json!({
                "reason": "compiler-artifact",
                "target": {"name": name, "kind": ["bin"]},
                "manifest_path": root.join("crates/ryframe/Cargo.toml"),
                "executable": executable,
                }))
                .unwrap()
            ),
        )
        .unwrap();
        fs::write(
            stderr,
            self.diagnostic
                .clone()
                .unwrap_or_else(|| format!("fixture build log {commit}")),
        )
        .unwrap();
        if self.fail_name.as_deref() == Some(name) {
            return Err("fixture build failed".into());
        }
        Ok(())
    }
}

#[test]
fn bridge_output_matches_python_and_rejects_drift_and_existing_output() {
    let fixture = Fixture::new();
    let (build, inventory) = fixture.restore_inputs();
    let rust_output = fixture.local.join("rust-bridge.json");
    let result = bridge::write(&fixture.root, &build, &inventory, &rust_output).unwrap();
    assert_eq!(result["status"], "artifact_reuse_audited");
    assert_eq!(result["compiled"], false);
    assert_eq!(result["restore_qualified"], false);

    let python_output = fixture.local.join("python-bridge.json");
    python(
        &fixture.root,
        r#"
import sys
from pathlib import Path
from source_fingerprints import write_bridge
write_bridge(Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3]), Path(sys.argv[4]))
"#,
        &[&fixture.root, &build, &inventory, &python_output],
    );
    assert_eq!(read_json(&rust_output), read_json(&python_output));

    let before = fs::read(&rust_output).unwrap();
    assert!(bridge::write(&fixture.root, &build, &inventory, &rust_output).is_err());
    assert_eq!(fs::read(&rust_output).unwrap(), before);

    write(&fixture.root.join("product/input.txt"), "changed\n");
    let changed = fixture.local.join("changed-bridge.json");
    let error = bridge::write(&fixture.root, &build, &inventory, &changed).unwrap_err();
    assert!(error.to_string().contains("重新编译"), "{error}");
    assert!(!changed.exists());
}

#[test]
fn bridge_rejects_wrong_source_and_linked_input() {
    let fixture = Fixture::new();
    let (build, inventory) = fixture.restore_inputs();
    let mut value = read_json(&inventory);
    value["source"]["snapshot"]["head"] = json!("c".repeat(40));
    value["guard"]["head"] = json!("c".repeat(40));
    write_json(&inventory, &value);
    let error = bridge::write(
        &fixture.root,
        &build,
        &inventory,
        &fixture.local.join("wrong-source.json"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("构建时"), "{error}");

    let fixture = Fixture::new();
    let (build, inventory) = fixture.restore_inputs();
    let linked = fixture.local.join("linked-inventory.json");
    if make_file_link(&inventory, &linked).is_err() {
        return;
    }
    let error = bridge::write(
        &fixture.root,
        &build,
        &linked,
        &fixture.local.join("linked-output.json"),
    )
    .unwrap_err();
    assert!(error.to_string().contains("链接"), "{error}");
}

#[test]
fn maintenance_build_receipt_verifies_read_only_and_matches_python() {
    let fixture = Fixture::new();
    let runner = FakeRunner::default();
    let output = fixture.local.join("maintenance");
    let receipt = maintenance::build_with(&fixture.root, &output, &runner).unwrap();
    assert_eq!(runner.calls.get(), 3);
    assert_eq!(
        runner.built.borrow().as_slice(),
        [
            "ryframe-reset".to_owned(),
            "ryframe-migrate".to_owned(),
            "ryframe-tenant-data".to_owned()
        ]
    );
    assert_eq!(receipt["format_version"], 1);
    assert_eq!(receipt["kind"], "devex-clone-tool-build");
    assert_eq!(receipt["resources_modified"], false);
    assert_eq!(receipt["restore_qualified"], false);
    assert_eq!(receipt["artifacts"].as_object().unwrap().len(), 3);
    assert_eq!(
        receipt["source_inventory"],
        source::inventory_value(&source::capture(&fixture.root).unwrap())
    );
    assert!(!output.join("failed.json").exists());
    assert_eq!(
        maintenance::build_summary(&output),
        json!({
            "status": "maintenance_build_created",
            "receipt": output.join("build.json"),
            "resources_modified": false,
            "restore_qualified": false,
        })
    );

    let build = output.join("build.json");
    let before = snapshot(&output);
    let verified = maintenance::verify_with(&fixture.root, &build, &runner).unwrap();
    assert_eq!(verified, receipt);
    assert_eq!(
        maintenance::verify_summary(&build),
        json!({
            "status": "maintenance_build_verified",
            "receipt": build,
            "resources_modified": false,
            "restore_qualified": false,
        })
    );
    assert_eq!(snapshot(&output), before);
    assert_eq!(runner.calls.get(), 3);

    let python_receipt = python_json(
        &fixture.root,
        r#"
import json
import sys
from pathlib import Path
from devex_clone_tools import TOOLS, target_directory, verify_evidence
root, build = Path(sys.argv[1]), Path(sys.argv[2])
observed = json.loads(build.read_text(encoding="utf-8"))
target = target_directory(root)
assert observed["backend_root"] == str(root), (observed["backend_root"], str(root))
assert observed["target_directory"] == str(target), (observed["target_directory"], str(target))
assert set(observed["artifacts"]) == set(TOOLS), (set(observed["artifacts"]), set(TOOLS))
print(json.dumps(verify_evidence(root, build), ensure_ascii=False))
"#,
        &[&fixture.root, &build],
    );
    assert_eq!(python_receipt, receipt);

    let calls = runner.calls.get();
    let error = maintenance::build_with(&fixture.root, &output, &runner).unwrap_err();
    assert!(error.to_string().contains("已存在"), "{error}");
    assert_eq!(runner.calls.get(), calls);
}

#[test]
fn maintenance_detects_input_and_receipt_tampering() {
    let fixture = Fixture::new();
    let runner = FakeRunner::default();
    let output = fixture.local.join("maintenance-drift");
    maintenance::build_with(&fixture.root, &output, &runner).unwrap();
    let build = output.join("build.json");

    let receipt = fs::read(&build).unwrap();
    let mut value = read_json(&build);
    value["artifacts"]["migrate"]["extra"] = json!(true);
    write_json(&build, &value);
    assert!(maintenance::verify_with(&fixture.root, &build, &runner).is_err());
    fs::write(&build, receipt).unwrap();

    let binary = PathBuf::from(
        read_json(&build)["artifacts"]["tenant-data"]["executable"]
            .as_str()
            .unwrap(),
    );
    fs::write(&binary, b"tampered").unwrap();
    let error = maintenance::verify_with(&fixture.root, &build, &runner).unwrap_err();
    assert!(error.to_string().contains("实际二进制"), "{error}");

    let fixture = Fixture::new();
    let runner = FakeRunner::default();
    let output = fixture.local.join("maintenance-source-drift");
    maintenance::build_with(&fixture.root, &output, &runner).unwrap();
    write(&fixture.root.join("product/input.txt"), "drifted\n");
    let error =
        maintenance::verify_with(&fixture.root, &output.join("build.json"), &runner).unwrap_err();
    assert!(error.to_string().contains("当前完整开发源码"), "{error}");
}

#[test]
fn maintenance_failure_preserves_stage_and_redacts_diagnostics() {
    let fixture = Fixture::new();
    let runner = FakeRunner {
        fail_name: Some("ryframe-migrate".into()),
        diagnostic: Some("token=fixture-secret\n".into()),
        ..FakeRunner::default()
    };
    let output = fixture.local.join("maintenance-failure");
    let error = maintenance::build_with(&fixture.root, &output, &runner).unwrap_err();
    assert!(error.to_string().contains("build_migrate"), "{error}");
    let failure = read_json(&output.join("failed.json"));
    assert_eq!(failure["status"], "failed");
    assert_eq!(failure["stage"], "build_migrate");
    assert_eq!(failure["resources_modified"], false);
    assert!(failure["source"].is_object());
    assert!(!output.join("build.json").exists());
    assert!(output.join("migrate.cargo.jsonl").is_file());
    assert!(output.join("migrate.cargo.log").is_file());

    let redacted =
        maintenance::redact_with("secret value=fixture secret", ["fixture secret".to_owned()]);
    assert_eq!(redacted, "secret value=[REDACTED]");
}

#[test]
fn maintenance_rejects_nonlocal_and_linked_output_before_running_commands() {
    let fixture = Fixture::new();
    let runner = FakeRunner::default();
    assert!(
        maintenance::build_with(&fixture.root, &fixture.root.join("outside"), &runner).is_err()
    );
    assert_eq!(runner.calls.get(), 0);

    let outside = fixture.root.join("target-outside");
    fs::create_dir_all(&outside).unwrap();
    let target = fixture.root.join("target");
    if make_directory_link(&outside, &target).is_err() {
        return;
    }
    let error =
        maintenance::build_with(&fixture.root, &fixture.local.join("linked-target"), &runner)
            .unwrap_err();
    assert!(error.to_string().contains("链接"), "{error}");
    assert_eq!(runner.calls.get(), 0);
}

fn python(root: &Path, script: &str, arguments: &[&Path]) {
    let result =
        Command::new(std::env::var_os("RYFRAME_PYTHON").unwrap_or_else(|| "python".into()))
            .args(["-B", "-c", script])
            .args(arguments)
            .current_dir(root)
            .env("PYTHONPATH", workspace::root_dir().join("tools/python"))
            .output()
            .unwrap();
    assert!(
        result.status.success(),
        "python failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

fn python_json(root: &Path, script: &str, arguments: &[&Path]) -> Value {
    let result =
        Command::new(std::env::var_os("RYFRAME_PYTHON").unwrap_or_else(|| "python".into()))
            .args(["-B", "-c", script])
            .args(arguments)
            .current_dir(root)
            .env("PYTHONPATH", workspace::root_dir().join("tools/python"))
            .output()
            .unwrap();
    assert!(
        result.status.success(),
        "python failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut values = fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.0.cmp(&right.0));
    values
}

fn git(root: &Path, arguments: &[&str]) {
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn write(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn write_json(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[cfg(unix)]
fn make_file_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn make_file_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(source, target)
}

#[cfg(unix)]
fn make_directory_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, target)
}

#[cfg(windows)]
fn make_directory_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(source, target)
}
