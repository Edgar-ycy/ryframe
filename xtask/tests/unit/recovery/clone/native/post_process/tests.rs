use std::fs;

use serde_json::json;

use super::{command, inspection, lock, protocol};

#[test]
fn producer_plan_keeps_credentials_out_of_argv() {
    let root = tempfile::tempdir().unwrap();
    let backend = root.path();
    let source = backend.join("xtask/src/data/performance_identities/producer.rs");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, "// source\n").unwrap();
    let tool = backend.join("xtask.exe");
    fs::write(&tool, "tool").unwrap();
    let request = json!({"backend":backend,"run_dir":backend.join("run"),"attempt":2,"kind":"identity-apply","output":backend.join("run/seed-runtime/attempt-0002"),"tool":{"path":tool,"sha256":"a".repeat(64)},"registration":{"identity_plan":{"path":backend.join("plan.json"),"sha256":"b".repeat(64)},"identity_state":backend.join("identities"),"password":"Private-Secret"},"configuration":null});
    let value = command::build(&request).unwrap();
    let encoded = serde_json::to_string(&value["argv"]).unwrap();
    assert!(!encoded.contains("Private-Secret"));
    assert!(encoded.contains("--identity-plan-sha256"));
}

#[test]
fn identity_release_is_permanent_and_idempotent() {
    let base = std::env::current_dir().unwrap().join(".local-tests/tmp");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("post-lock-")
        .tempdir_in(base)
        .unwrap();
    let backend = root.path();
    let state = backend.join("identities");
    fs::create_dir(&state).unwrap();
    let identity = json!({"pid":4000,"started":"1000","executable":backend.join("xtask.exe")});
    let owner = uuid::Uuid::new_v4().to_string();
    let plan = "c".repeat(64);
    let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":4000,"plan_sha256":plan,"owner_token":owner});
    fs::write(
        state.join("lock"),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    let request = json!({"backend":backend,"identity_state":state,"identity":identity,"owner_token":owner,"plan_sha256":plan});
    assert_eq!(lock::recover(&request).unwrap()["released"], true);
    let resumed = lock::recover(&request).unwrap();
    assert_eq!(resumed["released"], false);
    assert_eq!(resumed["idempotent"], true);
    assert!(!state.join("lock").exists());
}

#[test]
fn unsupported_kind_fails_before_a_command_is_built() {
    let root = tempfile::tempdir().unwrap();
    let request = json!({"backend":root.path(),"run_dir":root.path().join("run"),"attempt":1,"kind":"unknown","output":root.path().join("out"),"tool":{"path":root.path().join("tool"),"sha256":"d".repeat(64)},"registration":{},"configuration":null});
    assert!(command::build(&request).is_err());
}

#[test]
fn pid_reuse_only_accepts_a_strictly_newer_generation() {
    let old = json!({"pid":44,"started":"100","executable":"D:/tool.exe"});
    assert!(inspection::newer(
        &old,
        &json!({"pid":44,"started":"101","executable":"D:/other.exe"})
    ));
    assert!(!inspection::newer(
        &old,
        &json!({"pid":45,"started":"101","executable":"D:/tool.exe"})
    ));
    assert!(!inspection::newer(
        &old,
        &json!({"pid":44,"started":"99","executable":"D:/tool.exe"})
    ));
    assert!(!inspection::newer(
        &old,
        &json!({"pid":44,"started":"unknown","executable":"D:/tool.exe"})
    ));
}

#[test]
fn producer_argv_and_cleanup_refuse_a_reused_current_pid() {
    let current = std::process::id();
    let fake = json!({"pid":current,"started":"1","executable":std::env::current_exe().unwrap()});
    assert!(protocol::terminate_process(&json!({"identity":fake})).is_err());
    #[cfg(windows)]
    {
        let expected = json!(std::env::args().collect::<Vec<_>>());
        let error = inspection::require_actual_argv(&fake, &expected).unwrap_err();
        assert_eq!(error.to_string(), "登记进程已退出或内核创建身份变化");
    }
}

#[test]
fn owners_must_be_uuid_v4() {
    assert!(inspection::uuid(&json!(uuid::Uuid::new_v4().to_string())));
    assert!(!inspection::uuid(&json!(
        "10000000-0000-1000-8000-000000000001"
    )));
    assert!(!inspection::uuid(&json!(false)));
}

#[test]
fn producer_kind_is_bound_to_one_stage_and_mode() {
    assert!(command::accepts("identity-apply", "identities-apply"));
    assert!(!command::accepts("identity-apply", "identities-verify"));
    assert!(command::accepts("session", "reconcile"));
    assert!(!command::accepts("session", "verify"));
}

#[test]
fn identity_release_rejects_another_owner_without_modifying_lock() {
    let base = std::env::current_dir().unwrap().join(".local-tests/tmp");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("post-lock-owner-")
        .tempdir_in(base)
        .unwrap();
    let backend = root.path();
    let state = backend.join("identities");
    fs::create_dir(&state).unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let plan = "e".repeat(64);
    let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":4001,"plan_sha256":plan,"owner_token":owner});
    let bytes = format!("{}\n", serde_json::to_string_pretty(&value).unwrap());
    fs::write(state.join("lock"), &bytes).unwrap();
    let request = json!({"backend":backend,"identity_state":state,"identity":{"pid":4001,"started":"1001","executable":backend.join("xtask.exe")},"owner_token":other,"plan_sha256":plan});
    assert!(lock::recover(&request).is_err());
    assert_eq!(fs::read(state.join("lock")).unwrap(), bytes.as_bytes());
}

#[test]
fn identity_release_resumes_after_lock_claim_interruption() {
    let base = std::env::current_dir().unwrap().join(".local-tests/tmp");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("post-lock-claim-interrupt-")
        .tempdir_in(base)
        .unwrap();
    let backend = root.path();
    let state = backend.join("identities");
    let releases = state.join("lock-releases");
    fs::create_dir_all(&releases).unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let plan = "f".repeat(64);
    let identity = json!({"pid":4002,"started":"1002","executable":backend.join("xtask.exe")});
    let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":4002,"plan_sha256":plan,"owner_token":owner});
    let release = uuid::Uuid::new_v4();
    fs::write(
        releases.join(format!("{release}.lock")),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    let request = json!({"backend":backend,"identity_state":state,"identity":identity,"owner_token":owner,"plan_sha256":plan});
    let resumed = lock::recover(&request).unwrap();
    assert_eq!(resumed["released"], true);
    assert_eq!(resumed["idempotent"], false);
    assert!(releases.join(format!("{release}.released.json")).is_file());
}

#[test]
fn identity_release_resumes_after_pending_marker_interruption() {
    let base = std::env::current_dir().unwrap().join(".local-tests/tmp");
    fs::create_dir_all(&base).unwrap();
    let root = tempfile::Builder::new()
        .prefix("post-lock-marker-interrupt-")
        .tempdir_in(base)
        .unwrap();
    let backend = root.path();
    let state = backend.join("identities");
    fs::create_dir(&state).unwrap();
    let owner = uuid::Uuid::new_v4().to_string();
    let plan = "1".repeat(64);
    let identity = json!({"pid":4003,"started":"1003","executable":backend.join("xtask.exe")});
    let value = json!({"format_version":1,"kind":"devex-identity-run-lock","pid":4003,"plan_sha256":plan,"owner_token":owner});
    fs::write(
        state.join("lock"),
        format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    )
    .unwrap();
    let request = json!({"backend":backend,"identity_state":state,"identity":identity,"owner_token":owner,"plan_sha256":plan});
    assert_eq!(lock::recover(&request).unwrap()["released"], true);
    let releases = state.join("lock-releases");
    let marker = fs::read_dir(&releases)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".released.json"))
        .unwrap();
    let pending = marker.with_file_name(
        marker
            .file_name()
            .unwrap()
            .to_string_lossy()
            .replace(".released.json", ".released.json.pending"),
    );
    fs::rename(&marker, &pending).unwrap();
    let resumed = lock::recover(&request).unwrap();
    assert_eq!(resumed["released"], true);
    assert_eq!(resumed["idempotent"], false);
    assert!(marker.is_file());
    assert!(!pending.exists());
}
