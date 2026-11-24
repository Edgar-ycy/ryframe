use super::*;
use crate::recovery::clone::native::copy_owner::Role;

fn temporary() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join(".local-tests/target");
    fs::create_dir_all(&target).unwrap();
    (temp, target)
}

fn original() -> (tempfile::TempDir, Receipt, Value) {
    let (temp, target) = temporary();
    let controller = lock::ProcessGuard::acquire_named(&target, Role::Target.guard_name()).unwrap();
    fs::create_dir(target.join("initialize.lock")).unwrap();
    let observation = prepare_lock::observe(temp.path(), &target).unwrap();
    drop(controller);
    let earlier =
        json!({"number": 2, "stage": "post-copy", "mode": "register", "status": "passed"});
    let running = json!({"number": 3, "stage": "post-copy", "mode": "prepare", "status": "running",
        "result": null, "error_type": null, "finished_at": null});
    let saved = json!({"attempts":[earlier,running]});
    let receipt = Receipt {
        format_version: 1,
        kind: KIND.into(),
        path: String::new(),
        state_sha256: digest::plan_hash(&saved).unwrap(),
        attempt: running,
        controller: Value::Null,
        owner: Value::Null,
        context: PrepareContext {
            manifest: Value::Null,
            initialized: Value::Null,
            registration: Value::Null,
            attempt_directory: (0, 0),
        },
        lock: observation,
    };
    let mut interrupted = saved;
    interrupted["attempts"][1]["status"] = json!("failed");
    interrupted["attempts"][1]["error_type"] = json!("ControllerInterrupted");
    interrupted["attempts"][1]["finished_at"] = json!("2026-11-23T00:00:00Z");
    (temp, receipt, interrupted)
}

#[test]
fn only_the_original_interrupted_prepare_and_its_entire_history_are_accepted() {
    let (_temp, receipt, mut saved) = original();
    assert_eq!(interrupted_number(&saved, &receipt).unwrap(), 3);

    saved["attempts"][1]["error_type"] = json!("OtherFailure");
    assert!(interrupted_number(&saved, &receipt).is_err());
    saved["attempts"][1]["error_type"] = json!("ControllerInterrupted");
    saved["attempts"][1]["result"] = json!({"path":"unexpected"});
    assert!(interrupted_number(&saved, &receipt).is_err());
    saved["attempts"][1]["result"] = Value::Null;
    saved["attempts"][0]["status"] = json!("failed");
    assert!(interrupted_number(&saved, &receipt).is_err());
}

#[cfg(windows)]
#[test]
fn observation_after_exit_rejects_an_owner_that_is_still_live() {
    let (_temp, run) = temporary();
    let sha256 = "0".repeat(64);
    let manifest = json!({"sha256": sha256});
    let owner = json!({
        "format_version": 1,
        "identity": identity::current().unwrap(),
        "directory": manifest::path_text(&run, "合成运行目录").unwrap(),
        "manifest_sha256": manifest["sha256"],
    });
    require_process(&run, &owner, &manifest, true).unwrap();
    assert!(require_process(&run, &owner, &manifest, false).is_err());
}

#[test]
fn acquiring_recovery_never_creates_a_replacement_guard_or_accepts_a_new_directory() {
    let (temp, receipt, _) = original();
    let root = temp.path().join(".local-tests");
    let target = root.join("target");
    let old = target.join("initialize.lock");
    let run = root.join("run");
    fs::create_dir(&run).unwrap();
    let archive = run.join("archive");
    fs::rename(&old, root.join("kept-original")).unwrap();
    fs::create_dir(&old).unwrap();
    assert!(prepare_lock::Recovery::acquire(temp.path(), &target, &archive, receipt.lock).is_err());
    assert!(old.is_dir());
    assert!(!archive.exists());
    let guard = target.join(Role::Target.guard_name());
    fs::remove_file(&guard).unwrap();
    assert!(prepare_lock::Recovery::acquire(temp.path(), &target, &archive, receipt.lock).is_err());
    assert!(!guard.exists());
}

#[cfg(windows)]
#[test]
fn lost_result_requires_the_same_archived_directory_and_the_original_intent() {
    let (temp, receipt, _) = original();
    let root = temp.path().join(".local-tests");
    let target = root.join("target");
    let run = root.join("run");
    fs::create_dir(&run).unwrap();
    let archive = run.join("archive");
    let intent = run.join("original.recovering.json");
    let value = json!({"bound": "original"});
    {
        let recovery =
            prepare_lock::Recovery::acquire(temp.path(), &target, &archive, receipt.lock).unwrap();
        establish_intent(temp.path(), &intent, &value, recovery.location().unwrap()).unwrap();
        recovery.archive().unwrap();
        assert_eq!(recovery.location().unwrap(), Location::Archived);
        assert!(recovery.archive().is_err());
    }
    let recovery =
        prepare_lock::Recovery::acquire(temp.path(), &target, &archive, receipt.lock).unwrap();
    let location = recovery.location().unwrap();
    assert_eq!(location, Location::Archived);
    assert!(establish_intent(temp.path(), &intent, &json!({"bound":"other"}), location).is_err());
    establish_intent(temp.path(), &intent, &value, location).unwrap();
    assert!(!target.join("initialize.lock").exists());
    let unknown = run.join("unknown.recovering.json");
    assert!(establish_intent(temp.path(), &unknown, &value, location).is_err());
    assert!(!unknown.exists());
}
