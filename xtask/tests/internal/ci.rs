use super::cli::{CiCommand, ResourceGateReplayOptions};
use super::{
    check::{
        BackendSnapshotProfile, CheckPlanMode, TaskExecutor, VerifySelection, WorkspaceGraph,
        preflight_migration_args, tasks_for,
    },
    ci::{
        CiJob, ci_execution_plan_for, ci_execution_plan_for_profile, ci_plan_for,
        ci_selection_for_paths, integration_test_args_for_target, parse_changed_paths,
        plan_outputs,
        resource_gate::{delegates_generic_ci_path, should_run_for_paths},
        resource_gate_replay_args, resource_gate_required_for_ci_range, tls_integration_args,
        verify_frontend_checkout_ref, windows_check_args, windows_process_test_args,
    },
};
use std::path::Path;

fn planned(plan: &[CiJob], job: CiJob) -> bool {
    plan.contains(&job)
}

#[test]
fn resource_gate_replay_fixes_repository_paths_at_the_xtask_boundary() {
    let options = ResourceGateReplayOptions {
        manifest: "D:/证据/replay.json".into(),
        work_dir: "D:/后端/.local-tests/resource-gate-replay/run".into(),
        report: "D:/后端/.local-tests/resource-gate-replay/report.json".into(),
        activation_gate: true,
    };
    let arguments = resource_gate_replay_args(
        &options,
        Path::new("D:/后端 worktree"),
        Path::new("D:/前端 worktree"),
    )
    .unwrap();
    assert_eq!(
        arguments,
        [
            "tools/python/resource_gate_replay.py",
            "--repository",
            "D:/后端 worktree",
            "--frontend-repository",
            "D:/前端 worktree",
            "--manifest",
            "D:/证据/replay.json",
            "--work-dir",
            "D:/后端/.local-tests/resource-gate-replay/run",
            "--report",
            "D:/后端/.local-tests/resource-gate-replay/report.json",
            "--activation-gate",
        ]
    );
}

#[test]
fn pull_request_edit_rechecks_identity_policy_and_consumer_contract() {
    let plan = ci_plan_for("pull_request", "edited", &VerifySelection::default(), true).unwrap();

    assert!(planned(&plan, CiJob::Preflight));
    assert!(!planned(&plan, CiJob::RustGate));
    assert!(!planned(&plan, CiJob::ResourceGate));
    assert!(!planned(&plan, CiJob::Integration));
    assert!(planned(&plan, CiJob::ConsumerContract));
}

#[test]
fn documentation_change_runs_only_preflight() {
    let plan = ci_plan_for(
        "pull_request",
        "synchronize",
        &VerifySelection::default(),
        false,
    )
    .unwrap();

    assert!(planned(&plan, CiJob::Preflight));
    assert!(!planned(&plan, CiJob::RustGate));
    assert!(!planned(&plan, CiJob::ResourceGate));
    assert!(!planned(&plan, CiJob::Integration));
    assert!(!planned(&plan, CiJob::ConsumerContract));
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

    let plan = ci_plan_for("pull_request", "synchronize", &selection, false).unwrap();

    assert!(planned(&plan, CiJob::Preflight));
    assert!(planned(&plan, CiJob::RustGate));
    assert!(!planned(&plan, CiJob::ResourceGate));
    assert!(planned(&plan, CiJob::Integration));
    assert!(planned(&plan, CiJob::ConsumerContract));
}

#[test]
fn shared_pull_request_change_expands_to_full_plan() {
    let mut selection = VerifySelection::default();
    selection.full_reason = Some("共享配置变化".to_owned());
    let plan = ci_plan_for("pull_request", "synchronize", &selection, false).unwrap();

    assert!(planned(&plan, CiJob::Preflight));
    assert!(planned(&plan, CiJob::RustGate));
    assert!(planned(&plan, CiJob::ResourceGate));
    assert!(planned(&plan, CiJob::Integration));
    assert!(planned(&plan, CiJob::ConsumerContract));
}

#[test]
fn non_pr_events_run_full_backend_gates_without_consumer_contract() {
    for event in ["push", "schedule", "workflow_dispatch"] {
        let plan = ci_plan_for(event, "", &VerifySelection::default(), false).unwrap();
        assert!(planned(&plan, CiJob::Preflight), "{event}");
        assert!(planned(&plan, CiJob::RustGate), "{event}");
        assert!(planned(&plan, CiJob::ResourceGate), "{event}");
        assert!(planned(&plan, CiJob::Integration), "{event}");
        assert!(!planned(&plan, CiJob::ConsumerContract), "{event}");
    }
}

#[test]
fn resource_change_selects_the_resource_gate_without_unrelated_backend_work() {
    let plan = ci_plan_for(
        "pull_request",
        "synchronize",
        &VerifySelection::default(),
        true,
    )
    .unwrap();

    assert!(planned(&plan, CiJob::Preflight));
    assert!(!planned(&plan, CiJob::RustGate));
    assert!(planned(&plan, CiJob::ResourceGate));
    assert!(!planned(&plan, CiJob::Integration));
    assert!(!planned(&plan, CiJob::ConsumerContract));
}

#[test]
fn resource_aware_ci_selection_keeps_targeted_and_full_surfaces_distinct() {
    let graph = WorkspaceGraph::default();
    for (name, path) in [
        ("标准资源清单", "catalog/resources/post.toml"),
        ("ownership 事实源", "catalog/resources/.ownership.toml"),
        (
            "ownership 可解释的生成输出",
            "crates/ryframe-api/src/generated/post/mod.rs",
        ),
    ] {
        let paths = vec![path.to_owned()];
        let selection = ci_selection_for_paths(&paths, &graph);
        let plan = ci_plan_for(
            "pull_request",
            "synchronize",
            &selection,
            should_run_for_paths(&paths),
        )
        .unwrap();
        assert!(delegates_generic_ci_path(path), "{name}: {path}");
        assert!(selection.full_reason.is_none(), "{name}: {path}");
        assert!(planned(&plan, CiJob::Preflight), "{name}: {path}");
        assert!(!planned(&plan, CiJob::RustGate), "{name}: {path}");
        assert!(planned(&plan, CiJob::ResourceGate), "{name}: {path}");
        assert!(!planned(&plan, CiJob::Integration), "{name}: {path}");
        assert!(!planned(&plan, CiJob::ConsumerContract), "{name}: {path}");
    }

    for (name, path) in [
        (
            "生成器或模板",
            "crates/ryframe-generator/src/resource/render.rs",
        ),
        ("Cargo", "Cargo.lock"),
        ("工具链", "rust-toolchain.toml"),
        ("build.rs", "crates/ryframe-api/build.rs"),
        ("CI", ".github/workflows/ci.yml"),
        ("架构策略", "architecture/crate-boundaries.toml"),
    ] {
        let paths = vec![path.to_owned()];
        let selection = ci_selection_for_paths(&paths, &graph);
        let plan = ci_plan_for(
            "pull_request",
            "synchronize",
            &selection,
            should_run_for_paths(&paths),
        )
        .unwrap();
        assert!(!delegates_generic_ci_path(path), "{name}: {path}");
        assert!(selection.full_reason.is_some(), "{name}: {path}");
        assert!(planned(&plan, CiJob::Preflight), "{name}: {path}");
        assert!(planned(&plan, CiJob::RustGate), "{name}: {path}");
        assert!(planned(&plan, CiJob::ResourceGate), "{name}: {path}");
        assert!(planned(&plan, CiJob::Integration), "{name}: {path}");
        assert!(planned(&plan, CiJob::ConsumerContract), "{name}: {path}");
    }
}

#[test]
fn ci_plan_and_each_independent_job_share_the_same_task_specs() {
    let mut selection = VerifySelection::default();
    selection.full_reason = Some("共享工具变化".to_owned());
    let full = ci_plan_for("pull_request", "synchronize", &selection, false).unwrap();
    assert_eq!(
        plan_outputs(&full),
        [
            ("preflight", true),
            ("rust_gate", true),
            ("resource_gate", true),
            ("integration", true),
            ("consumer_contract", true),
        ]
    );
    for (command, expected) in [
        (
            CiCommand::Preflight,
            &[
                TaskExecutor::CargoFormat,
                TaskExecutor::PythonEnvironment,
                TaskExecutor::PythonTests,
                TaskExecutor::NodeTests,
                TaskExecutor::PolicyChecks,
                TaskExecutor::MigrationHistory,
            ][..],
        ),
        (
            CiCommand::RustGate,
            &[
                TaskExecutor::CiFrontendCheckout,
                TaskExecutor::SnapshotPrepare,
                TaskExecutor::FeatureRegistry,
                TaskExecutor::WorkspaceClippy,
                TaskExecutor::WorkspaceGates,
                TaskExecutor::SnapshotVerify,
            ][..],
        ),
        (CiCommand::ResourceGate, &[TaskExecutor::CiResourceGate][..]),
        (CiCommand::Integration, &[TaskExecutor::CiIntegration][..]),
        (
            CiCommand::ConsumerContract,
            &[
                TaskExecutor::CiFrontendCheckout,
                TaskExecutor::CiContractSource,
                TaskExecutor::FrontendDependencies,
                TaskExecutor::ConsumerContract,
            ][..],
        ),
    ] {
        let execution = ci_execution_plan_for_profile(&command, Some("standard")).unwrap();
        assert_eq!(
            execution
                .tasks
                .iter()
                .map(|task| task.executor)
                .collect::<Vec<_>>(),
            expected
        );
        for task in &execution.tasks {
            assert_eq!(task.definition().id, task.id);
        }
    }

    let check_full = tasks_for(super::cli::CheckScope::All, &CheckPlanMode::ExplicitFull).unwrap();
    let backend_full = tasks_for(
        super::cli::CheckScope::Backend,
        &CheckPlanMode::ExplicitFull,
    )
    .unwrap();
    for shared in [
        TaskExecutor::CargoFormat,
        TaskExecutor::PythonEnvironment,
        TaskExecutor::PythonTests,
        TaskExecutor::NodeTests,
        TaskExecutor::PolicyChecks,
        TaskExecutor::MigrationHistory,
        TaskExecutor::SnapshotPrepare,
        TaskExecutor::FeatureRegistry,
        TaskExecutor::WorkspaceClippy,
        TaskExecutor::WorkspaceGates,
        TaskExecutor::SnapshotVerify,
        TaskExecutor::FrontendDependencies,
        TaskExecutor::ConsumerContract,
    ] {
        assert!(
            check_full.tasks.iter().any(|task| task.executor == shared)
                || backend_full
                    .tasks
                    .iter()
                    .any(|task| task.executor == shared),
            "普通完整检查与 CI 应共用原子节点 {shared:?}"
        );
    }
}

#[test]
fn rust_gate_profile_only_changes_the_selected_primitive_nodes() {
    let standard = ci_execution_plan_for_profile(&CiCommand::RustGate, Some("standard")).unwrap();
    assert_eq!(standard.tasks[0].executor, TaskExecutor::CiFrontendCheckout);
    assert_eq!(standard.tasks[1].executor, TaskExecutor::SnapshotPrepare);

    let windows =
        ci_execution_plan_for_profile(&CiCommand::RustGate, Some("windows-smoke")).unwrap();
    assert_eq!(
        windows
            .tasks
            .iter()
            .map(|task| task.executor)
            .collect::<Vec<_>>(),
        [
            TaskExecutor::CiFrontendCheckout,
            TaskExecutor::CiWindowsSmoke,
        ]
    );
    assert!(
        ci_execution_plan_for_profile(&CiCommand::RustGate, Some("unknown"))
            .unwrap_err()
            .to_string()
            .contains("只允许 standard 或 windows-smoke")
    );
    assert!(
        ci_execution_plan_for_profile(&CiCommand::Preflight, Some("unknown")).is_ok(),
        "Rust gate profile 不得改变其他独立 job"
    );
    assert!(ci_execution_plan_for(&CiCommand::Plan).is_err());
}

#[test]
fn invalid_pull_request_range_schedules_command_level_full_fallback() {
    assert!(!resource_gate_required_for_ci_range("pull_request", true));
    assert!(resource_gate_required_for_ci_range("pull_request", false));
    assert!(!resource_gate_required_for_ci_range(
        "workflow_dispatch",
        false
    ));
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
            "tools/python/check_migration_history.py",
            "--require-frozen",
            "--trusted-ref",
            sha,
        ]
    );
    assert_eq!(
        preflight_migration_args(Some(&"0".repeat(40))),
        [
            "tools/python/check_migration_history.py",
            "--require-frozen"
        ]
    );
}

#[test]
fn integration_commands_share_the_ci_target_and_jobs() {
    assert_eq!(
        integration_test_args_for_target(
            "ryframe-db",
            "mysql_real_protocol",
            Some("repositories,migration"),
            "target/ci/backend",
            4,
        ),
        [
            "test",
            "--locked",
            "--target-dir",
            "target/ci/backend",
            "-p",
            "ryframe-db",
            "--features",
            "repositories,migration",
            "--test",
            "mysql_real_protocol",
            "--jobs",
            "4",
            "--",
            "--nocapture",
        ]
    );
    assert_eq!(
        integration_test_args_for_target(
            "ryframe-adapters",
            "redis_real_protocol",
            Some("redis-api"),
            "target/ci/backend",
            4,
        ),
        [
            "test",
            "--locked",
            "--target-dir",
            "target/ci/backend",
            "-p",
            "ryframe-adapters",
            "--features",
            "redis-api",
            "--test",
            "redis_real_protocol",
            "--jobs",
            "4",
            "--",
            "--nocapture",
        ]
    );
    assert_eq!(
        tls_integration_args("target/ci/backend", 4),
        [
            "tools/python/tls_integration_gate.py",
            "--backend-root",
            ".",
            "--target-dir",
            "target/ci/backend",
            "--jobs",
            "4",
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
