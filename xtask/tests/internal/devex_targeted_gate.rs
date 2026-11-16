use std::ffi::OsStr;

use super::*;

#[test]
fn targeted_gate_uses_distinct_historical_and_explicit_protocols() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("sample");
    let source = TargetedArm {
        backend_base: "a".repeat(40),
        backend_head: "b".repeat(40),
        frontend_base: "c".repeat(40),
        frontend_head: "d".repeat(40),
    };
    let mut baseline = Command::new("cargo");
    baseline.env("RYFRAME_WORKSPACE_ROOT", root.path().join("other-source"));
    baseline.env(
        "RYFRAME_DEVEX_TARGET_ROOT",
        root.path().join("other-target"),
    );
    let mut candidate = Command::new("cargo");
    bind(
        &mut baseline,
        root.path(),
        &target,
        "warmup",
        PairedArm::Baseline,
        &source,
    )
    .unwrap();
    bind(
        &mut candidate,
        root.path(),
        &target,
        "warmup",
        PairedArm::Candidate,
        &source,
    )
    .unwrap();
    let env: std::collections::BTreeMap<_, _> = baseline.get_envs().collect();
    assert_eq!(env[OsStr::new("RYFRAME_WORKSPACE_ROOT")], None);
    assert_eq!(env[OsStr::new("RYFRAME_DEVEX_TARGET_ROOT")], None);
    assert_eq!(
        env[OsStr::new("RYFRAME_CI_BASE_SHA")],
        Some(OsStr::new(&source.backend_base))
    );
    assert_eq!(
        env[OsStr::new("RYFRAME_CI_HEAD_SHA")],
        Some(OsStr::new(&source.backend_head))
    );
    assert_eq!(
        env[OsStr::new("RYFRAME_RESOURCE_GATE_DECISION_FILE")],
        Some(decision_path(&target, "warmup").as_os_str())
    );
    assert_eq!(baseline.get_args().count(), 0);
    assert!(
        candidate
            .get_envs()
            .filter(|(key, _)| LEGACY_GATE_ENVIRONMENT.iter().any(|name| key == name))
            .all(|(_, value)| value.is_none())
    );
    assert_eq!(
        candidate.get_args().collect::<Vec<_>>(),
        source.candidate_arguments().map(OsStr::new)
    );
}

#[test]
fn targeted_gate_preserves_full_and_mismatched_decisions_without_accepting_them() {
    let root = tempfile::tempdir().unwrap();
    let steps = vec!["resource-drift".to_owned()];
    let path = decision_path(root.path(), "measurement");
    for (mode, artifact_steps, fallback, accepted) in [
        ("targeted", "resource-drift", None, true),
        ("targeted", "resource-workspace", None, false),
        ("full", "resource-drift", Some("未知路径"), false),
    ] {
        fs::write(&path, serde_json::to_vec(&serde_json::json!({
            "formatVersion":1, "recognized":true, "mode":mode, "fallback":fallback, "steps":[artifact_steps]
        })).unwrap()).unwrap();
        let (decision, matched) =
            sample_decision(root.path(), "measurement", true, Some(&steps)).unwrap();
        assert_eq!(matched, accepted);
        assert_eq!(decision.unwrap().mode, mode);
        assert!(check_new_decision(root.path(), "measurement").is_err());
    }
}

#[test]
fn targeted_gate_cache_lock_is_per_checkout_and_requires_real_directories() {
    let baseline = tempfile::tempdir().unwrap();
    let candidate = tempfile::tempdir().unwrap();
    let lock = lock_cache(baseline.path()).unwrap();
    assert!(lock_cache(baseline.path()).is_err());
    assert!(lock_cache(candidate.path()).is_ok());
    drop(lock);
    assert!(lock_cache(baseline.path()).is_ok());
    fs::write(
        candidate.path().join("target/ci/resource"),
        b"not a directory",
    )
    .unwrap();
    assert!(verify_cache(candidate.path()).is_err());
}
