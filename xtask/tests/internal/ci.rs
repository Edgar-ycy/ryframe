use super::{
    check::{BackendSnapshotProfile, VerifySelection},
    ci::{
        ci_plan_for, formal_contract_source_args, integration_test_args, parse_changed_paths,
        preflight_migration_args, verify_frontend_checkout_ref, windows_check_args,
        windows_process_test_args,
    },
};
use std::path::Path;

#[test]
fn pull_request_edit_runs_only_consumer_contract() {
    let plan = ci_plan_for("pull_request", "edited", &VerifySelection::default());

    assert!(!plan.preflight);
    assert!(!plan.rust_gate);
    assert!(!plan.integration);
    assert!(plan.consumer_contract);
}

#[test]
fn documentation_change_runs_only_preflight() {
    let plan = ci_plan_for("pull_request", "synchronize", &VerifySelection::default());

    assert!(plan.preflight);
    assert!(!plan.rust_gate);
    assert!(!plan.integration);
    assert!(!plan.consumer_contract);
}

#[test]
fn database_change_selects_rust_integration_and_consumer_gates() {
    let mut selection = VerifySelection::default();
    selection.backend_packages = ["ryframe-db", "ryframe-api"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    selection.backend_snapshot_profiles = [
        BackendSnapshotProfile::Mysql,
        BackendSnapshotProfile::OpenApiContract,
    ]
    .into_iter()
    .collect();

    let plan = ci_plan_for("pull_request", "synchronize", &selection);

    assert!(plan.preflight);
    assert!(plan.rust_gate);
    assert!(plan.integration);
    assert!(plan.consumer_contract);
}

#[test]
fn shared_pull_request_change_expands_to_full_plan() {
    let mut selection = VerifySelection::default();
    selection.full_reason = Some("共享配置变化".to_owned());
    let plan = ci_plan_for("pull_request", "synchronize", &selection);

    assert!(plan.preflight);
    assert!(plan.rust_gate);
    assert!(plan.integration);
    assert!(plan.consumer_contract);
}

#[test]
fn non_pr_events_run_full_backend_gates_without_consumer_contract() {
    for event in ["push", "schedule", "workflow_dispatch"] {
        let plan = ci_plan_for(event, "", &VerifySelection::default());
        assert!(plan.preflight, "{event}");
        assert!(plan.rust_gate, "{event}");
        assert!(plan.integration, "{event}");
        assert!(!plan.consumer_contract, "{event}");
    }
}

#[test]
fn changed_paths_are_normalized_deduplicated_and_sorted() {
    assert_eq!(
        parse_changed_paths("crates\\db\\src.rs\nREADME.md\r\ncrates/db/src.rs\n"),
        ["README.md", "crates/db/src.rs"]
    );
}

#[test]
fn preflight_uses_only_a_valid_nonzero_base_sha() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        preflight_migration_args(Some(sha)),
        [
            "scripts/check_migration_history.py",
            "--require-frozen",
            "--trusted-ref",
            sha,
        ]
    );
    assert_eq!(
        preflight_migration_args(Some(&"0".repeat(40))),
        ["scripts/check_migration_history.py", "--require-frozen"]
    );
}

#[test]
fn integration_commands_share_the_ci_target_and_jobs() {
    assert_eq!(
        integration_test_args("ryframe-db", "mysql_real_protocol", 4),
        [
            "test",
            "--locked",
            "--target-dir",
            "target/ci/backend",
            "-p",
            "ryframe-db",
            "--test",
            "mysql_real_protocol",
            "--jobs",
            "4",
            "--",
            "--nocapture",
        ]
    );
}

#[test]
fn windows_smoke_commands_share_ci_targets_and_limit_test_jobs() {
    assert_eq!(
        windows_check_args("ryframe"),
        [
            "check",
            "--locked",
            "--target-dir",
            "target/ci/backend",
            "-p",
            "ryframe",
            "--all-targets",
        ]
    );
    assert_eq!(
        windows_process_test_args(4),
        [
            "test",
            "--locked",
            "--target-dir",
            "target/ci/backend",
            "-p",
            "xtask",
            "--test",
            "process_windows",
            "--jobs",
            "4",
            "--",
            "--nocapture",
        ]
    );
}

#[test]
fn frontend_checkout_requires_an_exact_requested_sha() {
    let requested = "0123456789abcdef0123456789abcdef01234567";
    assert!(verify_frontend_checkout_ref(requested, requested).is_ok());
    assert!(verify_frontend_checkout_ref("main", requested).is_ok());
    assert!(
        verify_frontend_checkout_ref(requested, "1123456789abcdef0123456789abcdef01234567")
            .is_err()
    );
    assert!(verify_frontend_checkout_ref("main", "not-a-commit").is_err());
}

#[test]
fn formal_source_command_receives_all_pinned_contract_inputs() {
    let frontend = Path::new("../frontend");
    let candidate = Path::new("artifacts/candidate-openapi.json");
    let head = "0123456789abcdef0123456789abcdef01234567";
    let args = formal_contract_source_args(frontend, head, "owner/backend", candidate);
    assert_eq!(
        args,
        [
            "scripts/verify_frontend_contract_source.py".to_owned(),
            "--backend-worktree".to_owned(),
            ".".to_owned(),
            "--backend-head".to_owned(),
            head.to_owned(),
            "--backend-repository".to_owned(),
            "owner/backend".to_owned(),
            "--source-metadata".to_owned(),
            frontend
                .join("openapi/source.json")
                .to_string_lossy()
                .into_owned(),
            "--frontend-openapi".to_owned(),
            frontend
                .join("openapi/openapi.json")
                .to_string_lossy()
                .into_owned(),
            "--candidate-openapi".to_owned(),
            candidate.to_string_lossy().into_owned(),
        ]
    );
}
