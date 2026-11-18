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

pub(super) fn arm_storage_bindings(storage_run: &Path) -> (Value, Value) {
    use native::lifecycle::state;

    fs::create_dir(storage_run).unwrap();
    let manifest = storage_run.join("manifest.json");
    write_json(&manifest, &json!({"kind":"devex-clone-run","fixture":true}));
    state::initialize(storage_run).unwrap();
    let storage_attempt = state::begin(
        storage_run,
        "storage-target",
        "restart",
        &json!({"fixture":true}),
    )
    .unwrap()
    .0;
    state::finish(
        storage_run,
        storage_attempt,
        Some(&json!({"status":"storage_restarted"})),
        None,
    )
    .unwrap();
    let historical = json!({
        "path": storage_run,
        "manifest": binding(&manifest),
        "state": binding(&storage_run.join("state.json")),
    });
    let cache_attempt = state::begin(
        storage_run,
        "cache-target",
        "restart",
        &json!({"fixture":true}),
    )
    .unwrap()
    .0;
    state::finish(
        storage_run,
        cache_attempt,
        Some(&json!({"status":"cache_restarted"})),
        None,
    )
    .unwrap();
    let evidence = (0..8)
        .map(|index| {
            let path = storage_run.join(format!("lifecycle-{index}.json"));
            write_json(&path, &json!({"fixture":index}));
            binding(&path)
        })
        .collect::<Vec<_>>();
    let effective = json!({
        "path": storage_run,
        "manifest": binding(&manifest),
        "storage": {
            "request": evidence[0],
            "storage": {
                "identity": {"pid": 1},
                "sha256": "a".repeat(64),
                "process_receipt": evidence[1],
                "launch_receipt": evidence[2],
            },
            "attempt": storage_attempt,
            "data_directory": storage_run.join("data"),
            "api_url": "http://127.0.0.1:1",
            "console_url": "http://127.0.0.1:2",
            "restart_result": binding(&state::result_path(storage_run, storage_attempt)),
        },
        "cache": {
            "request": evidence[3],
            "runtime": evidence[4],
            "redis": {"pid": 2},
            "owner": evidence[5],
            "controller": evidence[6],
            "attempt": cache_attempt,
            "ready": evidence[7],
            "restart_result": binding(&state::result_path(storage_run, cache_attempt)),
        },
    });
    (historical, effective)
}

#[test]
fn seed_to_arm_manifest_is_registered_by_the_native_state_machine() {
    let fixture = Fixture::new();
    let source_registration = fixture.local.join("source-registration.json");
    let target_registration = fixture.local.join("target-registration.json");
    let initialized_files = fixture.local.join("initialized-files.json");
    for path in [&source_registration, &initialized_files] {
        write_json(path, &json!({"fixture": true}));
    }
    let (historical, effective) = arm_storage_bindings(&fixture.local.join("target-storage-run"));
    write_json(&target_registration, &json!({"storage_run":historical}));
    let mut manifest = read_json(&fixture.manifest);
    manifest["copy_stage"] = json!("seed_to_arm");
    manifest["source_registration"] = binding(&source_registration);
    manifest["target_registration"] = binding(&target_registration);
    manifest["target_initialized_files"] = binding(&initialized_files);
    manifest["target_storage_run"] = effective;
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
fn cache_and_seed_cleanup_retain_old_failures_when_unrelated_evidence_is_unavailable() {
    use super::recovery::clone::native::lifecycle::state as ledger;
    for (stage_name, mode) in [
        ("cache-target", "stop"),
        ("cache-target", "recover"),
        ("seed-runtime", "stop"),
        ("seed-runtime", "recover"),
    ] {
        let fixture = Fixture::new();
        fixture.init().unwrap();
        let (number, _) = ledger::begin(&fixture.run, "copy", "run", &json!({})).unwrap();
        ledger::finish(
            &fixture.run,
            number,
            Some(&json!({"partial":true})),
            Some("UnknownWrite"),
        )
        .unwrap();
        let failure = fixture.run.join("failure-before.json");
        fs::write(&failure, b"original failure evidence").unwrap();
        let before = ledger::load(&fixture.run, false).unwrap()["attempts"][0].clone();
        let result = fixture.run.join("results/0001.json");
        fs::write(&result, b"unavailable original result").unwrap();
        fs::remove_file(fixture.local.join("source-request.json")).unwrap();
        for (stage_name, operation) in [
            ("cache-target", "resume"),
            ("storage-target", "stop"),
            ("copy", "run"),
        ] {
            assert!(
                stage::run_named(
                    &fixture.backend,
                    &fixture.run,
                    stage_name,
                    operation,
                    &Success
                )
                .is_err()
            );
            assert!(!fixture.run.join("unknown-write.txt").exists());
        }
        let published =
            stage::run_named(&fixture.backend, &fixture.run, stage_name, mode, &Success).unwrap();
        assert_eq!(published["attempt"], 2);
        let saved = ledger::load(&fixture.run, false).unwrap();
        assert_eq!(saved["attempts"][0], before);
        assert_eq!(saved["attempts"][1]["status"], "passed");
        assert_eq!(fs::read(&result).unwrap(), b"unavailable original result");
        assert_eq!(fs::read(&failure).unwrap(), b"original failure evidence");
        assert!(!fixture.run.join("run.lock").exists());
    }
}

#[test]
fn cache_and_seed_cleanup_record_source_capture_failure_without_relaxing_other_stages() {
    use super::recovery::clone::native::lifecycle::state as ledger;
    let fixture = Fixture::new();
    let backend = fixture.local.join("not-a-git-root");
    let run = backend.join(".local-tests/run");
    fs::create_dir_all(&run).unwrap();
    // 阻断 Git 向父级工作树查找，确保夹具真实触发来源采集失败。
    fs::write(backend.join(".git"), "invalid fixture git metadata\n").unwrap();
    let mut manifest = read_json(&fixture.manifest);
    for field in [
        "source_request",
        "initialized",
        "source_environment",
        "target_environment",
    ] {
        let original = Path::new(manifest[field]["path"].as_str().unwrap());
        let selected = backend.join(".local-tests").join(format!("{field}.json"));
        fs::copy(original, &selected).unwrap();
        manifest[field] = binding(&selected);
    }
    manifest["source_export"] = Value::Null;
    manifest["copy_directory"] = json!(backend.join(".local-tests/copy"));
    super::recovery::clone::native::lifecycle::manifest::validate(&manifest, &backend).unwrap();
    write_json(&run.join("manifest.json"), &manifest);
    ledger::initialize(&run).unwrap();
    assert!(super::recovery::clone::native::source::current(&backend).is_err());
    assert!(stage::run_named(&backend, &run, "copy", "run", &Success).is_err());
    assert!(
        ledger::load(&run, true).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(!run.join("unknown-write.txt").exists());
    for (stage_name, mode) in [
        ("cache-target", "recover"),
        ("seed-runtime", "stop"),
        ("seed-runtime", "recover"),
    ] {
        let result = stage::run_named(&backend, &run, stage_name, mode, &Success).unwrap();
        assert_eq!(result["stage"], stage_name);
        assert_eq!(result["mode"], mode);
    }
    let saved = ledger::load(&run, false).unwrap();
    let attempts = ledger::attempts(&saved).unwrap();
    assert_eq!(attempts.len(), 3);
    for attempt in attempts {
        assert_eq!(
            attempt["sources"],
            json!({"source_capture_error_type":"SourceCaptureError"})
        );
        assert_eq!(attempt["status"], "passed");
    }
    assert!(!run.join("run.lock").exists());
}

#[test]
fn failed_stage_exposes_typed_repair_guidance_without_echoing_arbitrary_errors() {
    for typed in [true, false] {
        let fixture = Fixture::new();
        fixture.init().unwrap();
        let error = stage::run_named(
            &fixture.backend,
            &fixture.run,
            "copy",
            "run",
            &DiagnosticFailure(typed),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(error.contains("0x80070005"), typed, "{error}");
        assert_eq!(error.contains("Winmgmt"), typed, "{error}");
        assert!(!error.contains("fixture-private-content"));
        assert!(error.contains(&fixture.run.join("failure-0001.json").display().to_string()));
        assert!(error.contains(&fixture.run.join("manifest.json").display().to_string()));
        assert!(error.contains("先核对完整前后像，不自动重试"));
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
fn status_preserves_registered_projection_and_is_read_only_with_matching_source() {
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
    assert_eq!(status["status"], "run_status");
    assert_eq!(status["id"], "clone-case");
    assert_eq!(status["attempts"][0]["status"], "passed");
    assert_eq!(
        status["attempts"][0]["result"],
        fixture.state()["attempts"][0]["result"]
    );
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

fn initialize_native_copy_fixture(fixture: &Fixture) -> Value {
    let mut manifest = read_json(&fixture.manifest);
    for (field, name) in [
        ("source_export", "source-export.json"),
        ("initialized", "initialized.json"),
    ] {
        let evidence = fixture.local.join(field);
        fs::create_dir(&evidence).unwrap();
        let path = evidence.join(name);
        fs::rename(fixture.local.join(name), &path).unwrap();
        manifest[field] = binding(&path);
    }
    for field in ["source_environment", "target_environment"] {
        let path = PathBuf::from(manifest[field]["path"].as_str().unwrap());
        write_json(&path, &json!({"environment":{"APP_ENV":"test"}}));
        manifest[field] = binding(&path);
    }
    write_json(&fixture.manifest, &manifest);
    fixture.init().unwrap();
    manifest
}

#[test]
fn native_copy_preflight_uses_registered_inputs_without_lock_or_attempt() {
    let fixture = Fixture::new();
    initialize_native_copy_fixture(&fixture);
    let before = snapshot(&fixture.local);
    let runner = native::copy_transfer::Runner;
    let request = stage::StageRequest {
        backend: &fixture.backend,
        run_dir: &fixture.run,
        stage: "copy",
        mode: "run",
        attempt: None,
    };
    stage::StageRunner::preflight(&runner, &request).unwrap();
    let error = stage::StageRunner::execute(&runner, &request).unwrap_err();
    assert_eq!(error.to_string(), "复制必须由持锁的正式阶段执行");
    let allocated = stage::StageRequest {
        attempt: Some(1),
        ..request
    };
    let error = stage::StageRunner::preflight(&runner, &allocated).unwrap_err();
    assert_eq!(error.to_string(), "复制只读预检必须在申请阶段序号之前执行");
    assert_eq!(snapshot(&fixture.local), before);
    assert!(!fixture.copy.exists());
    assert!(!fixture.run.join("run.lock").exists());
    assert_eq!(fixture.state()["attempts"], json!([]));
}

#[test]
fn native_copy_continuation_rejects_changed_plan_before_opening_resources() {
    let fixture = Fixture::new();
    let manifest = initialize_native_copy_fixture(&fixture);
    fs::create_dir(&fixture.copy).unwrap();
    let input = fixture.copy.join("input.json");
    let declaration = json!({"copy_stage":"source_to_seed"});
    write_json(&input, &declaration);
    let descriptor = binding(&input);
    let input_sha256 = native::digest::plan_hash(&declaration).unwrap();
    let mut plan = json!({"copy_stage":"source_to_seed","input_sha256":input_sha256});
    let original_plan = native::digest::plan_hash(&plan).unwrap();
    plan["plan_sha256"] = json!(original_plan);
    plan["copy_id"] = json!("changed");
    write_json(
        &fixture.copy.join("plan.json"),
        &json!({"format_version":1,"input_path":input,
            "input_file":{"bytes":descriptor["bytes"],"sha256":descriptor["sha256"]},
            "plan":plan}),
    );
    write_json(
        &fixture.copy.join("session.json"),
        &json!({"source_export":manifest["source_export"],"initialized":manifest["initialized"],
            "generation_sha256":"b".repeat(64),"plan_sha256":original_plan,
            "source_observation":{"shared-control":{},"shared":{},"dedicated-a":{},"dedicated-b":{}},
            "clone_verified":false,"restore_qualified":false}),
    );
    let before = snapshot(&fixture.local);
    for mode in ["reconcile", "resume"] {
        let request = stage::StageRequest {
            backend: &fixture.backend,
            run_dir: &fixture.run,
            stage: "copy",
            mode,
            attempt: None,
        };
        let error =
            stage::StageRunner::preflight(&native::copy_transfer::Runner, &request).unwrap_err();
        assert_eq!(
            error.to_string(),
            "原计划、固定输入或复制阶段与复制会话绑定不同"
        );
    }
    assert_eq!(snapshot(&fixture.local), before);
    assert!(!fixture.copy.join("ledger").exists());
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
