use super::*;
use std::fs;

#[test]
fn status_derived_from_all_three_observations_preserves_terminal_states() {
    let observe = |states: [&str; 3]| {
        json!({
            "api":{"state":states[0]},
            "worker":{"state":states[1]},
            "frontend":{"state":states[2]},
        })
    };
    for (persisted, states, expected) in [
        ("starting", ["running", "running", "running"], "running"),
        ("running", ["running", "stopped", "pending"], "degraded"),
        ("running", ["stopped", "stopped", "stopped"], "exited"),
        ("starting", ["pending", "stopped", "pending"], "interrupted"),
        (
            "start_failed",
            ["pending", "stopped", "stopped"],
            "start_failed",
        ),
        ("stopped", ["pending", "stopped", "stopped"], "stopped"),
        (
            "cleanup_failed",
            ["stopped", "stopped", "stopped"],
            "exited",
        ),
    ] {
        assert_eq!(derived_status(persisted, &observe(states)), expected);
    }
}

#[test]
fn status_distinguishes_first_creation_cut_and_registered_never_started() {
    let binding =
        json!({"registration":{"sha256":"a".repeat(64)},"target_plan":{"sha256":"b".repeat(64)}});
    let before = Bound {
        runtime: PathBuf::new(),
        binding: binding.clone(),
        registration: Value::Null,
        reference: PathBuf::new(),
        target: PathBuf::new(),
        endpoints: Value::Null,
        scope: Value::Null,
    };
    assert_eq!(
        before.operation("status").unwrap(),
        format!("runtime-status:{}:{}", "a".repeat(64), "b".repeat(64)),
    );
    assert_eq!(
        registration_reply("registered_not_started", &binding, None),
        json!({
            "format_version":1,"kind":"restore-runtime-lifecycle-result","operation":"status",
            "status":"registered_not_started","registration":binding,
        })
    );
    assert_eq!(
        registration_reply("interrupted_before_first_generation", &binding, Some(1)),
        json!({
            "format_version":1,"kind":"restore-runtime-lifecycle-result","operation":"status",
            "status":"interrupted_before_first_generation","registration":binding,"generation":1,
        })
    );
}

#[test]
fn status_rejects_non_file_receipts_and_detects_late_appearance() {
    let local = crate::workspace::root_dir().join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let temp = tempfile::tempdir_in(local).unwrap();
    let expected = temp.path().join(STATE);
    let window = Window::begin().unwrap();
    assert!(!optional_file(&expected).unwrap());
    fs::create_dir(&expected).unwrap();
    assert!(optional_file(&expected).is_err());
    assert!(window.finish(Ok(())).is_err());
}
