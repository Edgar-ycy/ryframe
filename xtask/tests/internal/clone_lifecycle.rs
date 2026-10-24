use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
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

struct DiagnosticFailure(bool);

impl stage::StageRunner for DiagnosticFailure {
    fn preflight(&self, _: &stage::StageRequest<'_>) -> Result<(), Box<dyn std::error::Error>> {
        Ok(())
    }

    fn execute(&self, _: &stage::StageRequest<'_>) -> Result<Value, Box<dyn std::error::Error>> {
        if self.0 {
            Err(Box::new(
                super::recovery::clone::native::seed::source_registration::quiet::CimUnavailable {
                    code: Some(-2147024891),
                },
            ))
        } else {
            Err("fixture-private-content Winmgmt 0x80070005".into())
        }
    }
}

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
        let status = super::recovery::clone::native::lifecycle::run(
            &CloneCommand::Status {
                run_dir: self.run.clone(),
            },
            &self.backend,
        )?;
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
fn seed_to_arm_manifest_is_registered_by_the_native_state_machine() {
    let fixture = Fixture::new();
    let source_registration = fixture.local.join("source-registration.json");
    let target_registration = fixture.local.join("target-registration.json");
    let initialized_files = fixture.local.join("initialized-files.json");
    let storage_run = fixture.local.join("target-storage-run");
    fs::create_dir(&storage_run).unwrap();
    let storage_manifest = storage_run.join("manifest.json");
    for path in [&source_registration, &initialized_files, &storage_manifest] {
        write_json(path, &json!({"fixture": true}));
    }
    super::recovery::clone::native::lifecycle::state::initialize(&storage_run).unwrap();
    let historical_state = binding(&storage_run.join("state.json"));
    let storage_attempt = super::recovery::clone::native::lifecycle::state::begin(
        &storage_run,
        "storage-target",
        "restart",
        &json!({"fixture": true}),
    )
    .unwrap()
    .0;
    super::recovery::clone::native::lifecycle::state::finish(
        &storage_run,
        storage_attempt,
        Some(&json!({"status": "storage_restarted"})),
        None,
    )
    .unwrap();
    write_json(
        &target_registration,
        &json!({
            "storage_run": {
                "path": storage_run,
                "manifest": binding(&storage_manifest),
                "state": historical_state,
            }
        }),
    );
    let evidence = (0..10)
        .map(|index| {
            let path = fixture.local.join(format!("lifecycle-{index}.json"));
            write_json(&path, &json!({"fixture": index}));
            binding(&path)
        })
        .collect::<Vec<_>>();
    let mut manifest = read_json(&fixture.manifest);
    manifest["copy_stage"] = json!("seed_to_arm");
    manifest["source_registration"] = binding(&source_registration);
    manifest["target_registration"] = binding(&target_registration);
    manifest["target_initialized_files"] = binding(&initialized_files);
    manifest["target_storage_run"] = json!({
        "path": storage_run,
        "manifest": binding(&storage_manifest),
        "storage": {
            "request": evidence[0],
            "storage": {
                "identity": {"pid": 1},
                "sha256": "a".repeat(64),
                "process_receipt": evidence[1],
                "launch_receipt": evidence[2],
            },
            "attempt": 1,
            "data_directory": storage_run.join("data"),
            "api_url": "http://127.0.0.1:1",
            "console_url": "http://127.0.0.1:2",
            "restart_result": evidence[3],
        },
        "cache": {
            "request": evidence[4],
            "runtime": evidence[5],
            "redis": {"pid": 2},
            "owner": evidence[6],
            "controller": evidence[7],
            "attempt": 1,
            "ready": evidence[8],
            "restart_result": evidence[9],
        },
    });
    write_json(&fixture.manifest, &manifest);

    let registered = fixture.init().unwrap();
    assert_eq!(registered["status"], "run_registered");
    assert_eq!(fixture.status().unwrap()["attempts"], json!([]));
}

#[test]
fn dead_controller_recovery_is_native_and_requires_reconciliation() {
    let fixture = Fixture::new();
    fixture.init().unwrap();
    super::recovery::clone::native::lifecycle::state::begin(
        &fixture.run,
        "copy",
        "resume",
        &json!({"fixture": true}),
    )
    .unwrap();
    let lock = fixture.run.join("run.lock");
    fs::create_dir(&lock).unwrap();
    let owner_path = lock.join("owner.json");
    write_json(
        &owner_path,
        &json!({
            "format_version": 1,
            "identity": {
                "pid": u32::MAX,
                "started": "dead-fixture",
                "executable": "dead-fixture",
            },
            "directory": fixture.run,
            "manifest_sha256": binding(&fixture.run.join("manifest.json"))["sha256"],
        }),
    );
    let result = super::recovery::clone::native::lifecycle::run(
        &CloneCommand::Recover {
            run_dir: fixture.run.clone(),
            owner_binding: owner_path,
        },
        &fixture.backend,
    )
    .unwrap();
    assert_eq!(result["status"], "controller_recovered");
    assert_eq!(result["copy_requires_reconciliation"], true);
    assert_eq!(fixture.state()["attempts"][0]["status"], "failed");
    assert_eq!(
        fixture.state()["attempts"][0]["error_type"],
        "ControllerInterrupted"
    );
    assert!(!lock.exists());
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
    assert_eq!(failure["format_version"], 2);
    assert_eq!(failure["operation_id"], "clone.export.run");
    assert!(failure.get("frames").is_none());
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
fn failed_stage_exposes_typed_repair_guidance_without_echoing_arbitrary_errors() {
    for typed in [true, false] {
        let fixture = Fixture::new();
        fixture.init().unwrap();
        let error = stage::run_named(
            &fixture.backend,
            &fixture.run,
            "seed-runtime",
            "source-register",
            &DiagnosticFailure(typed),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(error.contains("0x80070005"), typed, "{error}");
        assert_eq!(error.contains("Winmgmt"), typed, "{error}");
        assert!(!error.contains("fixture-private-content"));
        let state = fixture.state();
        assert_eq!(state["attempts"][0]["status"], "failed");
        assert_eq!(state["attempts"][0]["result"], Value::Null);
        assert_eq!(
            state["attempts"][0]["error_type"],
            if typed {
                "HistoricalProducerCimUnavailable"
            } else {
                "StageError"
            }
        );
        assert!(!fixture.run.join("run.lock").exists());
    }
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
fn native_status_preserves_the_registered_attempt_projection() {
    let fixture = Fixture::new();
    fixture.init().unwrap();
    stage::run(
        &fixture.backend,
        &fixture.run,
        super::cli::CloneStage::Export(super::cli::CloneStageMode::Run),
        &Success,
    )
    .unwrap();
    let native = fixture.status().unwrap();
    assert_eq!(native["attempts"][0]["execution_source_matches"], true);
    assert_eq!(native["status"], "run_status");
    assert_eq!(native["id"], "clone-case");
    assert_eq!(native["attempts"][0]["status"], "passed");
    assert_eq!(
        native["attempts"][0]["result"],
        fixture.state()["attempts"][0]["result"]
    );
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
    let mut protocol: Value =
        serde_json::from_str(&stage::protocol(&request, "preflight").unwrap()).unwrap();
    // libtest 不承载原生私有入口；适配夹具显式绑定同次构建的正式 xtask。
    let native = crate::check::python_suite::binding(Path::new(env!("CARGO_BIN_EXE_xtask")))
        .unwrap()
        .unwrap();
    protocol["native_tool"] = serde_json::from_str(&native).unwrap();
    let output = crate::process::command_output_with_input(
        &fixture.backend,
        "python",
        &["-B", "tools/python/devex_clone.py", "--private-stdin"],
        &serde_json::to_vec(&protocol).unwrap(),
    )
    .unwrap();
    let value: Value = serde_json::from_str(output.trim()).unwrap();
    assert_eq!(value["status"], "stage_operation_preflight_ready");
    assert_eq!(value["stage"], "export");
    assert_eq!(value["mode"], "run");
    assert!(!fixture.run.join("run.lock").exists());
    assert_eq!(fixture.state()["attempts"], json!([]));
}

#[test]
fn live_controller_owner_is_observable_by_native_status_without_writes() {
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
