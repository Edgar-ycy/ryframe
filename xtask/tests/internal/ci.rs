use std::path::Path;

use super::cli::{CheckCommand, CiCommand, Command, PreflightOptions, PreflightPart, parse};
use super::{
    check::{
        BackendSnapshotProfile, CheckPlanMode, TaskExecutor, VerifySelection, WorkspaceGraph,
        ci_target_policy_from, preflight_trusted_ref, tasks_for,
    },
    ci::{
        CiJob, ci_execution_plan_for, ci_execution_plan_for_profile, ci_plan_for,
        ci_selection_for_paths, integration_test_args_for_target, plan_outputs,
        resource_gate::{delegates_generic_ci_path, should_run_for_paths},
        resource_gate_required_for_ci_range, verify_frontend_checkout_ref, windows_check_args,
        windows_process_test_args,
    },
};
fn planned(plan: &[CiJob], job: CiJob) -> bool {
    plan.contains(&job)
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
fn generator_change_keeps_real_mysql_import_coverage() {
    let mut selection = VerifySelection::default();
    selection
        .backend_packages
        .insert("ryframe-generator".into());
    let plan = ci_plan_for("pull_request", "synchronize", &selection, false).unwrap();
    assert!(planned(&plan, CiJob::Integration));
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
            CiCommand::Preflight(Default::default()),
            &[
                TaskExecutor::CargoFormat,
                TaskExecutor::PythonEnvironment,
                TaskExecutor::PythonTests,
                TaskExecutor::BackendSources,
                TaskExecutor::PolicyChecks,
                TaskExecutor::PermissionRoutes,
                TaskExecutor::MigrationHistory,
            ][..],
        ),
        (
            CiCommand::RustGate(Default::default()),
            &[
                TaskExecutor::CiFrontendCheckout,
                TaskExecutor::SnapshotPrepare,
                TaskExecutor::FeatureRegistry,
                TaskExecutor::WorkspaceClippy,
                TaskExecutor::WorkspaceGates,
                TaskExecutor::SnapshotVerify,
            ][..],
        ),
        (
            CiCommand::ResourceGate(Default::default()),
            &[TaskExecutor::CiResourceGate][..],
        ),
        (CiCommand::Integration, &[TaskExecutor::CiIntegration][..]),
        (
            CiCommand::ConsumerContract(Default::default()),
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
    }
}

#[test]
fn ci_preflight_accepts_one_optional_part_and_preserves_base_sha() {
    let parse_command = |args: &[&str]| {
        parse(args.iter().map(|arg| (*arg).to_owned()).collect()).map(|cli| cli.command)
    };
    let base = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
    for (part, selected) in [
        ("checks", PreflightPart::Checks),
        ("python", PreflightPart::Python),
    ] {
        for flags in [
            ["--part", part, "--base-sha", base],
            ["--base-sha", base, "--part", part],
        ] {
            let mut args = vec!["check", "ci", "preflight"];
            args.extend(flags);
            assert_eq!(
                parse_command(&args).unwrap(),
                Command::Check(CheckCommand::Ci(CiCommand::Preflight(PreflightOptions {
                    base_sha: Some(base.to_ascii_lowercase()),
                    part: Some(selected)
                })))
            );
        }
    }
    assert!(
        parse_command(&[
            "check",
            "ci",
            "preflight",
            "--base-sha",
            "",
            "--part",
            "python"
        ])
        .is_ok()
    );
    for invalid in [
        &["--part"][..],
        &["--part", ""],
        &["--part", "all"],
        &["--part", "checks", "--part", "python"],
        &["--part", "checks", "--part", "checks"],
        &["--base-sha", "", "--base-sha", base, "--part", "python"],
    ] {
        let mut args = vec!["check", "ci", "preflight"];
        args.extend_from_slice(invalid);
        assert!(parse_command(&args).is_err(), "{args:?}");
    }
    assert!(parse_command(&["check", "ci", "plan", "--part", "checks"]).is_err());
}

#[test]
fn ci_preflight_parts_partition_the_original_job_and_keep_order() {
    let plan = |part| {
        ci_execution_plan_for(&CiCommand::Preflight(PreflightOptions {
            part,
            ..Default::default()
        }))
        .unwrap()
    };
    let full = plan(None);
    let checks = plan(Some(PreflightPart::Checks));
    let python = plan(Some(PreflightPart::Python));
    let ids = |plan: &crate::check::TaskPlan| {
        plan.tasks
            .iter()
            .map(|task| (task.id, task.executor))
            .collect::<Vec<_>>()
    };
    let full_ids = ids(&full);
    let python_ids = [
        ("python.environment", TaskExecutor::PythonEnvironment),
        ("python.tests", TaskExecutor::PythonTests),
    ];
    assert_eq!(ids(&python), python_ids);
    assert_eq!(python.tasks[1].dependencies, ["python.environment"]);
    assert_eq!(checks.tasks.len(), 5);
    assert!(
        ids(&checks).into_iter().eq(full_ids
            .iter()
            .copied()
            .filter(|task| !python_ids.contains(task)))
    );
    assert!(
        ids(&python).into_iter().eq(full_ids
            .iter()
            .copied()
            .filter(|task| python_ids.contains(task)))
    );
}

#[test]
fn daily_full_checks_use_explicit_repository_scopes() {
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
        TaskExecutor::BackendSources,
        TaskExecutor::PolicyChecks,
        TaskExecutor::DeploymentAssets,
        TaskExecutor::PermissionRoutes,
        TaskExecutor::MigrationHistory,
        TaskExecutor::SnapshotPrepare,
        TaskExecutor::FeatureRegistry,
        TaskExecutor::WorkspaceClippy,
        TaskExecutor::WorkspaceGates,
        TaskExecutor::SnapshotVerify,
        TaskExecutor::FrontendDependencies,
        TaskExecutor::FrontendConsumerContract,
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
    let command = CiCommand::RustGate(Default::default());
    let standard = ci_execution_plan_for_profile(&command, Some("standard")).unwrap();
    assert_eq!(standard.tasks[0].executor, TaskExecutor::CiFrontendCheckout);
    assert_eq!(standard.tasks[1].executor, TaskExecutor::SnapshotPrepare);

    let windows = ci_execution_plan_for_profile(&command, Some("windows-smoke")).unwrap();
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
        ci_execution_plan_for_profile(&command, Some("unknown"))
            .unwrap_err()
            .to_string()
            .contains("只允许 standard 或 windows-smoke")
    );
    assert!(
        ci_execution_plan_for_profile(&CiCommand::Preflight(Default::default()), Some("unknown"))
            .is_ok(),
        "Rust gate profile 不得改变其他独立 job"
    );
    assert!(ci_execution_plan_for(&CiCommand::Plan(None)).is_err());
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
fn preflight_uses_only_a_valid_nonzero_base_sha() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(preflight_trusted_ref(Some(sha)), Some(sha.to_owned()));
    assert_eq!(preflight_trusted_ref(Some(&"0".repeat(40))), None);
    assert_eq!(preflight_trusted_ref(Some("HEAD")), None);
    assert_eq!(preflight_trusted_ref(None), None);
}

#[test]
fn integration_commands_share_the_ci_target_and_jobs() {
    let planned: Vec<_> = crate::ci::INTEGRATION_TESTS
        .iter()
        .map(|&(package, target, features)| {
            integration_test_args_for_target(package, target, features, "target/ci/backend", 4)
        })
        .collect();
    let expected: Vec<_> = [
        (
            "ryframe-db",
            "mysql_real_protocol",
            "repositories,migration",
        ),
        ("ryframe-adapters", "redis_real_protocol", "redis-api"),
        ("ryframe-generator", "mysql_schema_import", "schema-import"),
        (
            "ryframe",
            "business_initialization_mysql",
            "bin-api,bin-migrate",
        ),
    ]
    .into_iter()
    .map(|(package, target, features)| {
        [
            "test",
            "--locked",
            "--no-default-features",
            "--target-dir",
            "target/ci/backend",
            "-p",
            package,
            "--features",
            features,
            "--test",
            target,
            "--jobs",
            "4",
            "--",
            "--nocapture",
        ]
        .map(str::to_owned)
        .to_vec()
    })
    .collect();
    assert_eq!(planned, expected);
    assert_eq!(
        crate::ci::tls::test_command(&crate::ci::tls::TESTS[0], Path::new("target/ci/backend"), 4),
        [
            "cargo",
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
            "tls_connection_round_trip_uses_real_redis",
            "--",
            "--exact",
            "--nocapture",
        ]
    );
}

#[test]
fn windows_smoke_commands_share_ci_targets_and_limit_test_jobs() {
    let targets = ci_target_policy_from(None);
    assert_eq!(targets.resource, "target/ci/resource");
    assert_eq!(
        windows_check_args("ryframe", &targets.backend),
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
        windows_process_test_args(&targets.backend, 4),
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

    let root = if cfg!(windows) {
        Path::new(r"D:\R")
    } else {
        Path::new("/r")
    };
    let targets = ci_target_policy_from(Some(root));
    assert_eq!(Path::new(&targets.backend), root.join("backend"));
    assert_eq!(Path::new(&targets.resource), root.join("resource"));
    assert_eq!(
        windows_check_args("ryframe", &targets.backend)[3],
        targets.backend
    );
    assert_eq!(
        windows_check_args("xtask", &targets.backend)[3],
        targets.backend
    );
    assert_eq!(
        windows_process_test_args(&targets.backend, 4)[3],
        targets.backend
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
