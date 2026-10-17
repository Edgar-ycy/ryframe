use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    cli::CloneCommand,
    recovery::clone::native::{self, lifecycle::stage},
    workspace,
};

static NEXT: AtomicUsize = AtomicUsize::new(1);

struct Fixture {
    backend: PathBuf,
    local: PathBuf,
    manifest: PathBuf,
    run: PathBuf,
    copy: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let backend = workspace::root_dir();
        let local = backend.join(".local-tests").join(format!(
            "clone-lifecycle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&local).unwrap();
        let request = local.join("source-request.json");
        write_json(&request, &json!({"kind": "fixture-source"}));
        let initialized = local.join("initialized.json");
        write_json(&initialized, &json!({"kind": "fixture-target"}));
        let source_environment = local.join("source-environment.json");
        write_json(&source_environment, &json!({"environment": {}}));
        let target_environment = local.join("target-environment.json");
        write_json(&target_environment, &json!({"environment": {}}));
        let exported = local.join("source-export.json");
        write_json(&exported, &json!({"request": binding(&request)}));
        let copy = local.join("copy");
        let manifest = local.join("manifest.json");
        write_json(
            &manifest,
            &json!({
                "format_version": 1,
                "kind": "devex-clone-run",
                "id": "clone-case",
                "source_request": binding(&request),
                "source_export": binding(&exported),
                "initialized": binding(&initialized),
                "source_environment": binding(&source_environment),
                "target_environment": binding(&target_environment),
                "copy_directory": copy,
                "copy_stage": "source_to_seed",
                "build_bridges": [],
            }),
        );
        Self {
            run: local.join("run"),
            backend,
            local,
            manifest,
            copy,
        }
    }

    fn init(&self) -> Result<Value, String> {
        super::recovery::clone::native::lifecycle::run(
            &CloneCommand::Init {
                manifest: self.manifest.clone(),
                run_dir: self.run.clone(),
            },
            &self.backend,
        )
        .map_err(|error| error.to_string())
    }

    fn status(&self) -> Result<Value, String> {
        super::recovery::clone::native::lifecycle::run(
            &CloneCommand::Status {
                run_dir: self.run.clone(),
            },
            &self.backend,
        )
        .map_err(|error| error.to_string())
    }

    fn state(&self) -> Value {
        read_json(&self.run.join("state.json"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        assert!(
            self.local
                .starts_with(workspace::root_dir().join(".local-tests"))
        );
        fs::remove_dir_all(&self.local).unwrap();
    }
}

struct Success;

impl stage::StageRunner for Success {
    fn preflight(
        &self,
        request: &stage::StageRequest<'_>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert!(request.attempt.is_none());
        Ok(())
    }

    fn execute(
        &self,
        request: &stage::StageRequest<'_>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        assert!(request.attempt.is_some());
        fs::write(request.run_dir.join("unknown-write.txt"), b"resource write").unwrap();
        Ok(json!({"status": "export_verified", "remote_writes": 0}))
    }
}

struct Failure;

impl stage::StageRunner for Failure {
    fn preflight(
        &self,
        _request: &stage::StageRequest<'_>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    fn execute(
        &self,
        request: &stage::StageRequest<'_>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        fs::write(request.run_dir.join("unknown-write.txt"), b"unknown").unwrap();
        Err("开发复制未完成：CancelledError".into())
    }
}

struct NeedsReconciliation;

impl stage::StageRunner for NeedsReconciliation {
    fn preflight(
        &self,
        _request: &stage::StageRequest<'_>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    fn execute(
        &self,
        _request: &stage::StageRequest<'_>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        Ok(json!({"status": "needs_reconciliation", "remote_writes": 0}))
    }
}

struct PreflightFailure;

impl stage::StageRunner for PreflightFailure {
    fn preflight(
        &self,
        _request: &stage::StageRequest<'_>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Err("fixture preflight failure".into())
    }

    fn execute(
        &self,
        _request: &stage::StageRequest<'_>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        panic!("预检失败后不能进入阶段执行")
    }
}

struct ObserveController {
    backend: PathBuf,
    run: PathBuf,
}

impl stage::StageRunner for ObserveController {
    fn preflight(
        &self,
        _request: &stage::StageRequest<'_>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    fn execute(
        &self,
        _request: &stage::StageRequest<'_>,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let status = python_status(&self.backend, &self.run);
        assert_eq!(status["controller"]["process_matches"], true, "{status}");
        assert_eq!(status["controller"]["process_missing"], false, "{status}");
        assert!(status["controller"].get("phase").is_none());
        Ok(json!({"status": "controller_observed"}))
    }
}

#[test]
fn init_status_and_stage_preserve_registered_output_shape() {
    let fixture = Fixture::new();
    let init = fixture.init().unwrap();
    assert_eq!(init["status"], "run_registered");
    assert_eq!(init["id"], "clone-case");
    assert_eq!(init["directory"], fixture.run.to_string_lossy().as_ref());
    assert_eq!(init["remote_writes"], 0);
    assert_eq!(init["stages_passed"], json!([]));
    assert_eq!(init["restore_qualified"], false);
    assert!(fixture.run.join("results").is_dir());
    assert!(fixture.run.join("manifest.json").is_file());

    let status = fixture.status().unwrap();
    assert_eq!(status["status"], "run_status");
    assert_eq!(status["id"], "clone-case");
    assert_eq!(status["controller"], Value::Null);
    assert_eq!(status["attempts"], json!([]));
    assert_eq!(status["remote_writes"], 0);
    assert_eq!(status["restore_qualified"], false);

    let result = stage::run(
        &fixture.backend,
        &fixture.run,
        super::cli::CloneStage::Export(super::cli::CloneStageMode::Run),
        &Success,
    )
    .unwrap();
    assert_eq!(result["status"], "stage_finished");
    assert_eq!(result["stage"], "export");
    assert_eq!(result["mode"], "run");
    assert_eq!(result["attempt"], 1);
    assert_eq!(result["result"]["status"], "export_verified");
    assert_eq!(result["restore_qualified"], false);
    assert!(!fixture.run.join("run.lock").exists());
    assert!(fixture.run.join("controller-0001.json").is_file());
    assert!(fixture.run.join("results/0001.json").is_file());
    assert!(fixture.copy.parent().unwrap().is_dir());

    let state = fixture.state();
    assert_eq!(state["attempts"][0]["number"], 1);
    assert_eq!(state["attempts"][0]["stage"], "export");
    assert_eq!(state["attempts"][0]["status"], "passed");
    assert_eq!(
        state::binding(&fixture.run.join("results").join("0001.json")),
        state["attempts"][0]["result"]
    );
}

#[test]
fn init_conflict_and_preflight_failure_do_not_overwrite_or_begin() {
    let fixture = Fixture::new();
    fixture.init().unwrap();
    let before = snapshot(&fixture.run);
    assert!(fixture.init().is_err());
    assert_eq!(snapshot(&fixture.run), before);

    let second = Fixture::new();
    second.init().unwrap();
    let attempts = second.state()["attempts"].clone();
    let error = stage::run(
        &second.backend,
        &second.run,
        super::cli::CloneStage::Copy(super::cli::CloneStageMode::Run),
        &PreflightFailure,
    )
    .unwrap_err();
    assert!(error.to_string().contains("fixture preflight failure"));
    assert_eq!(second.state()["attempts"], attempts);
    assert!(!second.run.join("run.lock").exists());
    assert!(!second.run.join("run-control.guard").exists());
}

#[test]
fn failure_cancel_and_partial_results_are_recorded_without_replay() {
    let fixture = Fixture::new();
    fixture.init().unwrap();
    let error = stage::run(
        &fixture.backend,
        &fixture.run,
        super::cli::CloneStage::Export(super::cli::CloneStageMode::Run),
        &Failure,
    )
    .unwrap_err();
    assert!(error.to_string().contains("CancelledError"));
    let state = fixture.state();
    assert_eq!(state["attempts"][0]["status"], "failed");
    assert_eq!(state["attempts"][0]["error_type"], "CancelledError");
    assert_eq!(state["attempts"][0]["result"], Value::Null);
    assert!(fixture.run.join("unknown-write.txt").is_file());
    assert!(!fixture.run.join("run.lock").exists());
    let failure = read_json(&fixture.run.join("failure-0001.json"));
    assert_eq!(failure["error_type"], "CancelledError");
    assert_eq!(failure["frames"], json!([]));
    assert_eq!(
        failure["controller"],
        state::binding(&fixture.run.join("controller-0001.json"))
    );

    let partial = Fixture::new();
    partial.init().unwrap();
    assert!(
        stage::run(
            &partial.backend,
            &partial.run,
            super::cli::CloneStage::Copy(super::cli::CloneStageMode::Reconcile),
            &NeedsReconciliation,
        )
        .is_err()
    );
    let state = partial.state();
    assert_eq!(state["attempts"][0]["status"], "failed");
    assert_eq!(state["attempts"][0]["error_type"], "NeedsReconciliation");
    assert_eq!(
        state["attempts"][0]["result"],
        state::binding(&partial.run.join("results").join("0001.json"))
    );
    assert_eq!(
        read_json(&partial.run.join("results").join("0001.json"))["status"],
        "needs_reconciliation"
    );
}

#[test]
fn revision_numbers_are_monotonic_and_tampering_fails_closed() {
    let fixture = Fixture::new();
    fixture.init().unwrap();
    stage::run(
        &fixture.backend,
        &fixture.run,
        super::cli::CloneStage::Export(super::cli::CloneStageMode::Run),
        &Success,
    )
    .unwrap();
    stage::run(
        &fixture.backend,
        &fixture.run,
        super::cli::CloneStage::Copy(super::cli::CloneStageMode::Reconcile),
        &Success,
    )
    .unwrap();
    let state = fixture.state();
    assert_eq!(state["attempts"][0]["number"], 1);
    assert_eq!(state["attempts"][1]["number"], 2);
    assert_eq!(state["attempts"][1]["status"], "passed");

    let state_path = fixture.run.join("state.json");
    let original = fs::read(&state_path).unwrap();
    let mut tampered = state;
    tampered["attempts"][1]["number"] = json!(9);
    write_json(&state_path, &tampered);
    assert!(fixture.status().is_err());
    fs::write(state_path, original).unwrap();
}

#[test]
fn status_is_read_only_and_reports_source_drift_without_controller_writes() {
    let fixture = Fixture::new();
    fixture.init().unwrap();
    stage::run(
        &fixture.backend,
        &fixture.run,
        super::cli::CloneStage::Export(super::cli::CloneStageMode::Run),
        &Success,
    )
    .unwrap();
    let before = snapshot(&fixture.run);
    let status = fixture.status().unwrap();
    assert_eq!(snapshot(&fixture.run), before);
    assert_eq!(status["attempts"][0]["execution_source_matches"], true);
    assert_eq!(status["controller"], Value::Null);
}

#[cfg(unix)]
#[test]
fn linked_manifest_is_rejected() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let linked = fixture.local.join("linked-manifest.json");
    symlink(&fixture.manifest, &linked).unwrap();
    let run = fixture.local.join("linked-run");
    assert!(
        native::run(
            &CloneCommand::Init {
                manifest: linked,
                run_dir: run.clone(),
            },
            &fixture.backend,
        )
        .is_err()
    );
    assert!(!run.exists());
}

#[cfg(windows)]
#[test]
fn linked_manifest_is_rejected() {
    use std::os::windows::fs::symlink_file;

    let fixture = Fixture::new();
    let linked = fixture.local.join("linked-manifest.json");
    if let Err(error) = symlink_file(&fixture.manifest, &linked) {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            return;
        }
        panic!("无法建立链接夹具：{error}");
    }
    let run = fixture.local.join("linked-run");
    assert!(
        native::run(
            &CloneCommand::Init {
                manifest: linked,
                run_dir: run.clone(),
            },
            &fixture.backend,
        )
        .is_err()
    );
    assert!(!run.exists());
}

#[test]
fn native_status_matches_python_field_for_field() {
    let fixture = Fixture::new();
    let native_init = fixture.init().unwrap();
    stage::run(
        &fixture.backend,
        &fixture.run,
        super::cli::CloneStage::Export(super::cli::CloneStageMode::Run),
        &Success,
    )
    .unwrap();
    let native = fixture.status().unwrap();
    assert_eq!(native["attempts"][0]["execution_source_matches"], true);
    let python_run = fixture.local.join("python-run");
    let protocol = json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-clone",
        "request": {
            "backend_dir": fixture.backend,
            "command": "init",
            "effect": "evidence-write",
            "options": {
                "manifest": fixture.manifest,
                "run_dir": python_run,
            },
            "write": true,
        }
    });
    let output = Command::new("python")
        .args(["-B", "tools/python/devex_clone.py"])
        .current_dir(&fixture.backend)
        .env(
            "RYFRAME_XTASK_RECOVERY_CLONE",
            serde_json::to_string(&protocol).unwrap(),
        )
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python_init: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(python_init["status"], "run_registered");
    assert_eq!(python_init["id"], native_init["id"]);

    let status_protocol = json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-clone",
        "request": {
            "backend_dir": fixture.backend,
            "command": "status",
            "effect": "read-only",
            "options": {"run_dir": fixture.run},
            "write": false,
        }
    });
    let output = Command::new("python")
        .args(["-B", "tools/python/devex_clone.py"])
        .current_dir(&fixture.backend)
        .env(
            "RYFRAME_XTASK_RECOVERY_CLONE",
            serde_json::to_string(&status_protocol).unwrap(),
        )
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let python_status: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(python_status, native);
}

#[test]
fn stage_python_preflight_uses_the_typed_resource_protocol() {
    let fixture = Fixture::new();
    fixture.init().unwrap();
    let request = stage::StageRequest {
        backend: &fixture.backend,
        run_dir: &fixture.run,
        stage: "export",
        mode: "run",
        attempt: None,
    };
    stage::StageRunner::preflight(&stage::PythonRunner::new(&fixture.backend), &request).unwrap();
    assert!(!fixture.run.join("run.lock").exists());
    assert_eq!(fixture.state()["attempts"], json!([]));
}

#[test]
fn live_controller_owner_is_observable_by_python_without_writes() {
    let fixture = Fixture::new();
    fixture.init().unwrap();
    stage::run(
        &fixture.backend,
        &fixture.run,
        super::cli::CloneStage::Export(super::cli::CloneStageMode::Run),
        &ObserveController {
            backend: fixture.backend.clone(),
            run: fixture.run.clone(),
        },
    )
    .unwrap();
    assert_eq!(fixture.state()["attempts"][0]["status"], "passed");
    assert!(!fixture.run.join("run.lock").exists());
}

fn binding(path: &Path) -> Value {
    let bytes = fs::read(path).unwrap();
    json!({
        "path": path.to_string_lossy(),
        "bytes": bytes.len(),
        "sha256": hex::encode(Sha256::digest(&bytes)),
    })
}

fn write_json(path: &Path, value: &Value) {
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    fs::write(path, text).unwrap();
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn python_status(backend: &Path, run: &Path) -> Value {
    let protocol = json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-clone",
        "request": {
            "backend_dir": backend,
            "command": "status",
            "effect": "read-only",
            "options": {"run_dir": run},
            "write": false,
        }
    });
    let output = Command::new("python")
        .args(["-B", "tools/python/devex_clone.py"])
        .current_dir(backend)
        .env(
            "RYFRAME_XTASK_RECOVERY_CLONE",
            serde_json::to_string(&protocol).unwrap(),
        )
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                result.insert(path.clone(), fs::read(path).unwrap());
            }
        }
    }
    result
}

mod state {
    pub(super) use super::binding;
}
