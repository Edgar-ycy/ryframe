use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    recovery::clone::native::{bridge, digest, maintenance, source},
    workspace,
};

static NEXT: AtomicUsize = AtomicUsize::new(1);

#[path = "clone_bridge/reuse.rs"]
mod reuse;

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
        Self::at(root)
    }

    fn at(root: PathBuf) -> Self {
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
                        "--features", feature, "--bin", name, "--jobs", "4", "--message-format=json"
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
                        "--features", "bin-api", "--bin", "ryframe", "--jobs", "4", "--message-format=json"
                    ],
                    "worker": [
                        "cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                        "--features", "bin-worker", "--bin", "ryframe-worker", "--jobs", "4", "--message-format=json"
                    ],
                },
                "profile": "dev",
                "target": "x86_64-test",
                "jobs": "4",
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
    source_drift: bool,
    toolchain_drift: bool,
}

impl maintenance::Runner for FakeRunner {
    fn version(
        &self,
        _root: &Path,
        executable: &str,
        _arguments: &[&str],
    ) -> Result<String, Box<dyn std::error::Error>> {
        if self.toolchain_drift && self.calls.get() > 0 {
            return Ok(format!("{executable} changed fixture"));
        }
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
        let target = arguments
            .iter()
            .position(|value| value == "--target-dir")
            .and_then(|index| arguments.get(index + 1))
            .unwrap();
        let executable = Path::new(target)
            .join("debug")
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
        if self.source_drift && self.calls.get() == 1 {
            fs::write(root.join("product/input.txt"), "changed during build\n")?;
        }
        Ok(())
    }
}

#[test]
fn source_paths_accept_valid_names_and_keep_domains() {
    for (path, domain) in [
        ("crates/业务 模块/src/file..rs", "product"),
        (".hidden", "product"),
        ("crates/.hidden/file..rs", "product"),
        ("file..rs", "product"),
        ("tests/中文 空格/.hidden", "test_tools"),
        ("xtask/tests/file..rs", "test_tools"),
        ("crates/业务 模块/tests/file..rs", "test_tools"),
        ("tools/中文 空格/check..py", "test_tools"),
        (".github/workflows/中文 空格.yml", "test_tools"),
        ("docs/中文 空格/.hidden", "support"),
        ("README.md", "support"),
        ("README.en.md", "support"),
    ] {
        let records = vec![source::FileRecord {
            path: path.into(),
            sha256: "a".repeat(64),
        }];
        let fingerprints = source::fingerprints(&records).unwrap();
        for group in ["product", "test_tools", "support"] {
            assert_eq!(
                fingerprints[group]["files"],
                json!(usize::from(group == domain)),
                "{path}: {group}"
            );
        }
        assert_eq!(
            fingerprints[domain]["sha256"],
            json!(digest::plan_hash(&json!([{"path": path, "sha256": "a".repeat(64)}])).unwrap()),
            "{path}"
        );
        let inventory = source::validate_inventory(&source_path_inventory(path)).unwrap();
        assert_eq!(inventory.files, records, "{path}");
        assert_eq!(inventory.source.snapshot.files, records, "{path}");
    }
}

#[test]
fn source_paths_reject_aliases_and_nonrelative_names() {
    for path in [
        "",
        ".",
        "..",
        "a//b",
        "a/./b",
        "a/../b",
        "a/b/",
        "a/b//",
        "a/.",
        "a/..",
        "./a/b",
        "../a/b",
        "/a/b",
        "//server/share/file.rs",
        "a\\b",
        "\\a\\b",
        "C:/a/b",
        "C:a/b",
        "z:/a/b",
        "z:a/b",
        "tests//file.rs",
        "xtask/tests/./file.rs",
        "crates/example/tests/file.rs/",
        "tools//check.py",
        ".github/./workflow.yml",
        "docs//guide.md",
        "README.md/",
    ] {
        let records = [source::FileRecord {
            path: path.into(),
            sha256: "a".repeat(64),
        }];
        assert!(source::fingerprints(&records).is_err(), "{path:?}");
        for pointer in ["/files/0/path", "/source/snapshot/files/0/path"] {
            let mut inventory = source_path_inventory("valid/file.rs");
            *inventory.pointer_mut(pointer).unwrap() = json!(path);
            assert!(
                source::validate_inventory(&inventory).is_err(),
                "{path:?}: {pointer}"
            );
        }
    }
}

#[test]
fn translated_readme_changes_keep_product_inputs_and_complete_source_evidence() {
    let before = source::validate_inventory(&source_path_inventory("README.en.md")).unwrap();
    let mut after = before.clone();
    after.files[0].sha256 = "9".repeat(64);
    after.source.snapshot.files[0].sha256 = "9".repeat(64);
    let before = source::backend_domains(&before).unwrap();
    let after = source::backend_domains(&after).unwrap();
    assert_eq!(before["product"], after["product"]);
    assert_eq!(before["tools"], after["tools"]);
    assert_ne!(before["full"], after["full"]);
}

fn source_path_inventory(path: &str) -> Value {
    let file = json!({"path": path, "sha256": "a".repeat(64)});
    json!({
        "source": {
            "snapshot": {
                "head": "b".repeat(40),
                "patch_sha256": "c".repeat(64),
                "files": [file],
                "clean": false,
            },
            "worktree_fingerprint": format!("sha256:{}", "d".repeat(64)),
        },
        "files": [file],
        "guard": {
            "head": "b".repeat(40),
            "index_sha256": "e".repeat(64),
            "modes_sha256": "f".repeat(64),
        },
    })
}

#[test]
fn bridge_preserves_receipt_and_rejects_product_drift_and_overwrite() {
    let fixture = Fixture::new();
    let (build, inventory) = fixture.restore_inputs();
    let build_bytes = fs::read(&build).unwrap();
    let inventory_bytes = fs::read(&inventory).unwrap();
    let build_receipt: Value = serde_json::from_slice(&build_bytes).unwrap();
    let original_inventory =
        source::validate_inventory(&serde_json::from_slice(&inventory_bytes).unwrap()).unwrap();
    let original_fingerprints = source::fingerprints(&original_inventory.files).unwrap();

    write(
        &fixture.root.join("tools/python/check.py"),
        "# 工具文件变更\n",
    );
    let audited_inventory = source::capture(&fixture.root).unwrap();
    let audited_source = source::execution_source(&audited_inventory).unwrap();
    assert!(!audited_inventory.source.snapshot.clean);
    assert_ne!(
        original_fingerprints["test_tools"],
        audited_source["fingerprints"]["test_tools"]
    );

    let output = fixture.local.join("bridge.json");
    assert_eq!(
        bridge::write(&fixture.root, &build, &inventory, &output).unwrap(),
        json!({
            "status": "artifact_reuse_audited",
            "receipt": output,
            "compiled": false,
            "restore_qualified": false,
        })
    );
    assert_eq!(
        read_json(&output),
        json!({
            "format_version": 1,
            "kind": "devex-artifact-source-bridge",
            "backend_root": fixture.root,
            "build": {"path": build, "sha256": hex::encode(Sha256::digest(&build_bytes))},
            "inventory": {"path": inventory, "sha256": hex::encode(Sha256::digest(&inventory_bytes))},
            "original_fingerprints": original_fingerprints,
            "audited_source": audited_source,
            "audited_inventory": source::inventory_value(&audited_inventory),
            "product_inputs": {
                "build": build_receipt["sources"]["product"],
                "audited": build_receipt["sources"]["product"],
            },
            "restore_qualified": false,
            "compiled": false,
        })
    );

    let before = fs::read(&output).unwrap();
    let error = bridge::write(&fixture.root, &build, &inventory, &output).unwrap_err();
    assert!(error.to_string().contains("拒绝覆盖"), "{error}");
    assert_eq!(fs::read(&output).unwrap(), before);

    write(&fixture.root.join("product/input.txt"), "changed\n");
    let changed = fixture.local.join("changed-bridge.json");
    let error = bridge::write(&fixture.root, &build, &inventory, &changed).unwrap_err();
    assert!(error.to_string().contains("重新编译"), "{error}");
    assert!(!changed.exists());
    assert_eq!(fs::read(&build).unwrap(), build_bytes);
    assert_eq!(fs::read(&inventory).unwrap(), inventory_bytes);
}

#[test]
fn bridge_rejects_noncanonical_v2_api_artifact_fields() {
    let fixture = Fixture::new();
    let (build, inventory) = fixture.restore_inputs();
    let mut receipt = read_json(&build);
    let executable = receipt["artifacts"]["api"]["executable"].clone();
    receipt["artifacts"]["api"]["cargo_executable"] = executable;
    write_json(&build, &receipt);

    let output = fixture.local.join("bridge.json");
    let error = bridge::write(&fixture.root, &build, &inventory, &output).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("API/Worker 构建产物字段缺失或包含未知字段"),
        "{error}"
    );
    assert!(!output.exists());
}

#[test]
fn bridge_rejects_same_length_api_artifact_change() {
    let fixture = Fixture::new();
    let (build, inventory) = fixture.restore_inputs();
    let receipt = read_json(&build);
    let executable = Path::new(receipt["artifacts"]["api"]["executable"].as_str().unwrap());
    assert_eq!(fs::read(executable).unwrap(), b"fixed api");
    fs::write(executable, b"other api").unwrap();

    let output = fixture.local.join("bridge.json");
    let error = bridge::write(&fixture.root, &build, &inventory, &output).unwrap_err();
    assert!(error.to_string().contains("构建收据不一致"), "{error}");
    assert!(!output.exists());
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
fn maintenance_build_records_file_jobs_and_verifies_frozen_artifacts_without_rebuild() {
    let fixture = Fixture::new();
    write(
        &fixture.root.join("config/tools.toml"),
        "[generator]\ntarget_dir='target/public-cache'\n",
    );
    write(
        &fixture.local.join("tools.toml"),
        "[generator]\ntarget_dir='target/shared-cache'\n[verification]\nbuild_jobs=1\n",
    );
    let runner = FakeRunner::default();
    let output = fixture.local.join("maintenance");
    let receipt = maintenance::build_with(&fixture.root, &output, &runner).unwrap();
    for role in ["reset", "migrate", "tenant-data"] {
        assert_eq!(receipt["artifacts"][role]["command"][11], "1");
    }
    assert_eq!(
        receipt["target_directory"],
        json!(fixture.root.join("target").join("shared-cache"))
    );
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
    assert_maintenance_cache_independence(&fixture, &output, &receipt, &runner);
    assert_eq!(
        maintenance::verify_summary(&build),
        json!({
            "status": "maintenance_build_verified",
            "receipt": build,
            "resources_modified": false,
            "restore_qualified": false,
        })
    );

    let calls = runner.calls.get();
    let error = maintenance::build_with(&fixture.root, &output, &runner).unwrap_err();
    assert!(error.to_string().contains("已存在"), "{error}");
    assert_eq!(runner.calls.get(), calls);

    write(
        &fixture.local.join("tools.toml"),
        "[generator]\ntarget_dir='target/shared-cache'\n[verification]\nbuild_jobs=2\n",
    );
    maintenance::verify_evidence(&fixture.root, &build, Some(&receipt)).unwrap();
    assert!(maintenance::verify_with(&fixture.root, &build, &runner).is_err());
    assert_eq!(runner.calls.get(), calls);
    write(
        &fixture.local.join("tools.toml"),
        "[generator]\ntarget_dir='target/shared-cache'\n[verification]\nbuild_jobs=0\n",
    );
    let invalid = fixture.local.join("invalid-jobs");
    assert!(maintenance::build_with(&fixture.root, &invalid, &runner).is_err());
    assert!(!invalid.exists());
    assert_eq!(runner.calls.get(), calls);
}

fn assert_maintenance_cache_independence(
    fixture: &Fixture,
    output: &Path,
    receipt: &Value,
    runner: &FakeRunner,
) {
    let build = output.join("build.json");
    let frozen = snapshot(output);
    let cache = receipt["artifacts"]
        .as_object()
        .unwrap()
        .values()
        .map(|item| PathBuf::from(item["cargo_executable"].as_str().unwrap()))
        .collect::<Vec<_>>();
    let target = Path::new(receipt["target_directory"].as_str().unwrap());
    for phase in 0..3 {
        if phase == 1 {
            for path in &cache {
                fs::remove_file(path).unwrap();
            }
            fs::remove_dir(cache[0].parent().unwrap()).unwrap();
            fs::remove_dir(target).unwrap();
        } else if phase == 2 {
            fs::create_dir_all(cache[0].parent().unwrap()).unwrap();
            for path in &cache {
                fs::write(path, b"later unrelated cached build").unwrap();
            }
        }
        let before = cache
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        let target_exists = target.exists();
        assert_eq!(
            maintenance::verify_evidence(&fixture.root, &build, Some(receipt)).unwrap(),
            *receipt
        );
        assert_eq!(
            maintenance::verify_with(&fixture.root, &build, runner).unwrap(),
            *receipt
        );
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
        assert_eq!(python_receipt, *receipt);
        assert_eq!(snapshot(output), frozen);
        assert_eq!(
            cache
                .iter()
                .map(|path| fs::read(path).ok())
                .collect::<Vec<_>>(),
            before
        );
        assert_eq!(target.exists(), target_exists);
        assert_eq!(runner.calls.get(), 3);
    }
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

    let original = read_json(&build);
    let events = output.join("reset.cargo.jsonl");
    let original_events = fs::read(&events).unwrap();
    for invalid in ["relative", "outside", "parent", "foreign", "duplicate"] {
        let mut event: Value = serde_json::from_slice(&original_events).unwrap();
        let mut changed = original.clone();
        match invalid {
            "relative" => event["executable"] = json!("relative.exe"),
            "outside" => event["executable"] = json!(fixture.local.join("missing.exe")),
            "parent" => {
                event["executable"] = json!(output.join("..").join("missing.exe"));
            }
            "foreign" => event["manifest_path"] = json!(fixture.root.join("Cargo.toml")),
            _ => {}
        }
        let line = format!("{event}\n");
        fs::write(
            &events,
            line.repeat(if invalid == "duplicate" { 2 } else { 1 }),
        )
        .unwrap();
        let file = digest::file(&events).unwrap();
        changed["artifacts"]["reset"]["cargo_output"]["bytes"] = json!(file.bytes);
        changed["artifacts"]["reset"]["cargo_output"]["sha256"] = json!(file.sha256);
        changed["artifacts"]["reset"]["cargo_executable"] = event["executable"].clone();
        write_json(&build, &changed);
        assert!(
            maintenance::verify_evidence(&fixture.root, &build, Some(&changed)).is_err(),
            "{invalid}"
        );
    }
    fs::write(&events, original_events).unwrap();
    write_json(&build, &original);
    let cache = Path::new(
        original["artifacts"]["reset"]["cargo_executable"]
            .as_str()
            .unwrap(),
    );
    let cached_bytes = fs::read(cache).unwrap();
    fs::remove_file(cache).unwrap();
    make_file_link(
        Path::new(
            original["artifacts"]["reset"]["executable"]
                .as_str()
                .unwrap(),
        ),
        cache,
    )
    .expect("维护缓存链接拒绝场景需要本地符号链接权限，不能静默跳过");
    assert!(maintenance::verify_evidence(&fixture.root, &build, Some(&original)).is_err());
    fs::remove_file(cache).unwrap();
    fs::write(cache, cached_bytes).unwrap();

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

    for drift in ["SourceDrift", "ToolchainDrift"] {
        let fixture = Fixture::new();
        let runner = FakeRunner {
            source_drift: drift == "SourceDrift",
            toolchain_drift: drift == "ToolchainDrift",
            ..FakeRunner::default()
        };
        let output = fixture.local.join(drift);
        let error = maintenance::build_with(&fixture.root, &output, &runner).unwrap_err();
        assert!(
            error.to_string().contains("verify_source_and_toolchain"),
            "{drift}: {error}"
        );
        let failure = read_json(&output.join("failed.json"));
        assert_eq!(failure["status"], "failed", "{drift}");
        assert_eq!(failure["stage"], "verify_source_and_toolchain", "{drift}");
        assert_eq!(failure["resources_modified"], false, "{drift}");
        assert!(failure["source"].is_object(), "{drift}");
        assert!(!output.join("build.json").exists(), "{drift}");
        for (role, name) in [
            ("reset", "ryframe-reset"),
            ("migrate", "ryframe-migrate"),
            ("tenant-data", "ryframe-tenant-data"),
        ] {
            assert!(
                output
                    .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
                    .is_file(),
                "{drift}"
            );
            assert!(
                output.join(format!("{role}.cargo.jsonl")).is_file(),
                "{drift}"
            );
            assert!(
                output.join(format!("{role}.cargo.log")).is_file(),
                "{drift}"
            );
        }
    }
}

#[test]
fn maintenance_rejects_nonlocal_and_linked_output_before_running_commands() {
    let fixture = Fixture::new();
    let runner = FakeRunner::default();
    assert!(
        maintenance::build_with(&fixture.root, &fixture.root.join("outside"), &runner).is_err()
    );
    assert_eq!(runner.calls.get(), 0);

    write(
        &fixture.local.join("tools.toml"),
        "[generator]\ntarget_dir=7\n",
    );
    let invalid = fixture.local.join("invalid-config");
    assert!(maintenance::build_with(&fixture.root, &invalid, &runner).is_err());
    assert!(!invalid.exists());
    assert_eq!(runner.calls.get(), 0);
    fs::remove_file(fixture.local.join("tools.toml")).unwrap();

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

fn python_json(root: &Path, script: &str, arguments: &[&Path]) -> Value {
    let result = python_output(root, script, arguments);
    assert!(
        result.status.success(),
        "python failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

fn python_output(root: &Path, script: &str, arguments: &[&Path]) -> std::process::Output {
    let tool = crate::rate_limit::tool_binding(Path::new(env!("CARGO_BIN_EXE_xtask")))
        .unwrap()
        .expect("维护收据互验必须绑定当前原生工具");
    let script = format!(
        "import sys\nfrom rate_limit_tool import install_verified_binding\ninstall_verified_binding(sys.argv.pop(1))\n{script}"
    );
    Command::new(
        crate::check::consumer_context::python::selected_program(&workspace::root_dir()).unwrap(),
    )
    .args(["-B", "-c", &script, &tool])
    .args(arguments)
    .current_dir(root)
    .env("PYTHONPATH", workspace::root_dir().join("tools/python"))
    .output()
    .unwrap()
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
