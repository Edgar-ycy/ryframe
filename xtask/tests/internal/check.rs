use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::Path,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use super::check::{
    BACKEND_CI_TARGET_DIR, BACKEND_POLICY_SCRIPTS, BACKEND_SMART_TARGET_DIR,
    BACKEND_VERIFY_TARGET_DIR, BackendSnapshotProfile, CONSUMER_OWNED_COMMANDS, ChangeCategory,
    ChangeSurfacePolicy, FRONTEND_FULL_NON_CONSUMER_COMMANDS, FRONTEND_ONLY_CONTRACT_COMMANDS,
    FrontendProfile, PYTHON_TEST_ARGS, RESOURCE_CI_TARGET_DIR, RESOURCE_VERIFY_TARGET_DIR,
    SMART_BACKEND_OPERATIONS, SMART_FEATURE_OPERATIONS, VerifyTargetPolicy, WORKSPACE_CLIPPY_ARGS,
    WorkspaceGraph, analyze_change_surface, append_changed_file_size_warnings,
    backend_package_operation_args, backend_snapshot_export_args, cargo_operation_jobs,
    changed_paths, ci_environment_from, ci_test_jobs_from, classify_changes,
    complete_verify_selection, consumer_contract_arguments, consumer_contract_plan,
    default_test_jobs_from, feature_operation_args, feature_test_args, frontend_profile_commands,
    load_change_surface_policy, load_consumer_contract_plan, load_workspace_graph,
    minimal_workspace_check_args, needs_consumer_contract, package_tests_generate_snapshots,
    resolve_target_dir, resource_test_executable_from_messages, reverse_dependency_closure,
    validate_feature_combination, verify_job_budget_from, verify_target_policy_from,
    workspace_clippy_args, workspace_test_args,
};

static NEXT_REPOSITORY: AtomicU64 = AtomicU64::new(1);

fn graph() -> WorkspaceGraph {
    WorkspaceGraph {
        package_by_dir: [
            ("crates/kernel".into(), "ryframe-kernel".into()),
            ("crates/application".into(), "ryframe-application".into()),
            ("crates/api".into(), "ryframe-api".into()),
            ("crates/db".into(), "ryframe-db".into()),
        ]
        .into_iter()
        .collect(),
        reverse_dependencies: [
            (
                "ryframe-kernel".into(),
                ["ryframe-application".into(), "ryframe-db".into()]
                    .into_iter()
                    .collect(),
            ),
            (
                "ryframe-application".into(),
                ["ryframe-api".into()].into_iter().collect(),
            ),
        ]
        .into_iter()
        .collect(),
    }
}

#[test]
fn feature_combination_rejects_duplicates_and_unknowns() {
    let available = ["a".to_owned(), "b".to_owned()]
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert!(
        validate_feature_combination("demo", "最小", &["a".into(), "a".into()], &available)
            .is_err()
    );
    assert!(validate_feature_combination("demo", "最小", &["c".into()], &available).is_err());
}

#[test]
fn feature_matrix_compiles_and_tests_required_feature_targets() {
    let features = vec![
        "destructive-reset".to_owned(),
        "file-maintenance".to_owned(),
    ];
    let check = feature_operation_args("check", "ryframe", &features, "target", 8);
    let clippy = feature_operation_args("clippy", "ryframe", &features, "target", 8);

    for args in [&check, &clippy] {
        assert!(args.windows(2).any(|pair| pair == ["-p", "ryframe"]));
        assert!(
            args.windows(2)
                .any(|pair| { pair == ["--target-dir", "target"] })
        );
        assert!(args.contains(&"--all-targets".to_owned()));
        assert!(args.contains(&"--no-default-features".to_owned()));
        assert!(args.windows(2).any(|pair| pair == ["--jobs", "8"]));
        assert!(args.contains(&"destructive-reset,file-maintenance".to_owned()));
    }
    let test = feature_test_args(
        "ryframe",
        &features,
        "reset_contract",
        "target",
        default_test_jobs_from(true, 8),
    );
    assert!(test.windows(2).any(|pair| pair == ["-p", "ryframe"]));
    assert!(test.windows(2).any(|pair| pair == ["--jobs", "4"]));
    assert!(
        test.windows(2)
            .any(|pair| { pair == ["--target-dir", "target"] })
    );
    assert!(
        test.windows(2)
            .any(|pair| pair == ["--test", "reset_contract"])
    );
    assert!(!test.contains(&"--all-targets".to_owned()));
    assert!(test.contains(&"--no-default-features".to_owned()));
    assert!(test.contains(&"destructive-reset,file-maintenance".to_owned()));
    assert!(clippy.ends_with(&[
        "--".into(),
        "-D".into(),
        "warnings".into(),
        "-D".into(),
        "clippy::redundant_clone".into(),
    ]));
}

#[test]
fn full_gate_discovers_repository_python_tests() {
    assert_eq!(
        PYTHON_TEST_ARGS,
        [
            "-m",
            "unittest",
            "discover",
            "-s",
            "scripts/tests",
            "-p",
            "test_*.py",
        ]
    );
    assert!(
        BACKEND_POLICY_SCRIPTS.contains(&"scripts/check_deployment_assets.py")
            && BACKEND_POLICY_SCRIPTS.contains(&"scripts/check_supply_chain.py")
    );
    assert_eq!(
        WORKSPACE_CLIPPY_ARGS,
        [
            "clippy",
            "--locked",
            "--target-dir",
            "target/verify/backend",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
            "-D",
            "clippy::redundant_clone",
        ]
    );
    assert_eq!(
        workspace_test_args("target/verify/backend", 4),
        [
            "test",
            "--locked",
            "--target-dir",
            "target/verify/backend",
            "--workspace",
            "--all-features",
            "--jobs",
            "4",
        ]
    );
    assert_eq!(BACKEND_SMART_TARGET_DIR, "target");
    assert_eq!(BACKEND_VERIFY_TARGET_DIR, "target/verify/backend");
    assert_eq!(BACKEND_CI_TARGET_DIR, "target/ci/backend");
    assert_eq!(RESOURCE_VERIFY_TARGET_DIR, "target/verify/resource");
    assert_eq!(RESOURCE_CI_TARGET_DIR, "target/ci/resource");
    assert_eq!(
        minimal_workspace_check_args("target/verify/backend", 8),
        [
            "check",
            "--locked",
            "--target-dir",
            "target/verify/backend",
            "--workspace",
            "--no-default-features",
            "--all-targets",
            "--jobs",
            "8",
        ]
    );
}

#[test]
fn verify_target_policy_distinguishes_smart_full_and_ci_targets() {
    assert!(!ci_environment_from(None));
    assert!(!ci_environment_from(Some(OsStr::new(""))));
    assert!(ci_environment_from(Some(OsStr::new("true"))));
    assert_eq!(
        verify_target_policy_from(false, false, None),
        VerifyTargetPolicy {
            backend: "target".to_owned(),
            resource: "target/verify/resource".to_owned(),
        }
    );
    assert_eq!(
        verify_target_policy_from(false, false, Some(Path::new("target/custom-smart")),),
        VerifyTargetPolicy {
            backend: "target/custom-smart".to_owned(),
            resource: "target/verify/resource".to_owned(),
        }
    );
    assert_eq!(
        verify_target_policy_from(true, false, Some(Path::new("target/ignored"))),
        VerifyTargetPolicy {
            backend: "target/verify/backend".to_owned(),
            resource: "target/verify/resource".to_owned(),
        }
    );
    assert_eq!(
        verify_target_policy_from(false, true, Some(Path::new("target/ignored"))),
        VerifyTargetPolicy {
            backend: "target/ci/backend".to_owned(),
            resource: "target/ci/resource".to_owned(),
        }
    );
    assert_eq!(
        verify_target_policy_from(true, true, Some(Path::new("target/ignored"))),
        VerifyTargetPolicy {
            backend: "target/ci/backend".to_owned(),
            resource: "target/ci/resource".to_owned(),
        }
    );
    assert_eq!(
        workspace_clippy_args("target/custom-backend", 8),
        [
            "clippy",
            "--locked",
            "--target-dir",
            "target/custom-backend",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--jobs",
            "8",
            "--",
            "-D",
            "warnings",
            "-D",
            "clippy::redundant_clone",
        ]
    );
}

#[test]
fn target_resolution_preserves_absolute_cargo_target_dir() {
    let root = std::env::current_dir().unwrap();
    let absolute = root.join("target/external-smart");
    let policy = verify_target_policy_from(false, false, Some(&absolute));

    assert_eq!(policy.backend, absolute.to_string_lossy());
    assert_eq!(
        resolve_target_dir(Path::new("ignored-root"), &policy.backend),
        absolute
    );
    assert_eq!(
        resolve_target_dir(Path::new("repository"), "target/custom-smart"),
        Path::new("repository").join("target/custom-smart")
    );
}

#[test]
fn smart_backend_uses_clippy_and_test_without_redundant_check() {
    assert_eq!(SMART_BACKEND_OPERATIONS, ["clippy", "test"]);
    assert_eq!(SMART_FEATURE_OPERATIONS, ["clippy"]);
    let packages = ["ryframe-api".to_owned(), "ryframe-db".to_owned()]
        .into_iter()
        .collect();
    let clippy = backend_package_operation_args(
        "clippy",
        &packages,
        "target",
        cargo_operation_jobs("clippy", true, 6),
    );
    let test = backend_package_operation_args(
        "test",
        &packages,
        "target",
        cargo_operation_jobs("test", true, 6),
    );
    for args in [&clippy, &test] {
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--target-dir", "target"])
        );
        assert!(!args.contains(&"check".to_owned()));
    }
    assert!(clippy.windows(2).any(|pair| pair == ["--jobs", "6"]));
    assert!(test.windows(2).any(|pair| pair == ["--jobs", "4"]));
    assert!(clippy.contains(&"--all-targets".to_owned()));
    assert!(!test.contains(&"--all-targets".to_owned()));
}

#[test]
fn windows_tests_use_four_jobs_without_reducing_compile_jobs() {
    assert_eq!(default_test_jobs_from(true, 12), 4);
    assert_eq!(default_test_jobs_from(true, 3), 4);
    assert_eq!(default_test_jobs_from(false, 12), 12);
    assert_eq!(cargo_operation_jobs("clippy", true, 12), 12);
    assert_eq!(cargo_operation_jobs("test", true, 12), 4);
    assert_eq!(ci_test_jobs_from(None, true, 12).unwrap(), 4);
    assert_eq!(ci_test_jobs_from(None, false, 12).unwrap(), 12);
    assert_eq!(ci_test_jobs_from(Some("6"), true, 12).unwrap(), 6);
    assert!(ci_test_jobs_from(Some("0"), true, 12).is_err());
}

#[test]
fn verify_job_budget_reserves_capacity_for_both_cargo_branches() {
    assert_eq!(
        verify_job_budget_from(None, 22).unwrap(),
        super::check::VerifyJobBudget {
            total: 12,
            backend: 8,
            resource: 4,
        }
    );
    assert_eq!(
        verify_job_budget_from(Some("9"), 22).unwrap(),
        super::check::VerifyJobBudget {
            total: 9,
            backend: 6,
            resource: 3,
        }
    );
    assert!(verify_job_budget_from(Some("3"), 22).is_err());
    assert!(verify_job_budget_from(Some("invalid"), 22).is_err());
}

#[test]
fn resource_test_executable_is_read_from_cargo_json_messages() {
    let output = concat!(
        "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"other\"},\"executable\":null}\n",
        "{\"reason\":\"compiler-artifact\",\"target\":{\"name\":\"resource_workspace_compilation\"},",
        "\"executable\":\"D:\\\\target\\\\resource-test.exe\"}\n"
    );
    assert_eq!(
        resource_test_executable_from_messages(output).unwrap(),
        Path::new("D:\\target\\resource-test.exe")
    );
}

#[test]
fn backend_snapshots_reuse_the_backend_verify_target() {
    assert_eq!(
        backend_snapshot_export_args(
            "target",
            "ryframe-api",
            "export_openapi",
            Path::new("target/xtask/openapi.json"),
        ),
        [
            "run",
            "--locked",
            "--target-dir",
            "target",
            "-p",
            "ryframe-api",
            "--bin",
            "export_openapi",
            "--",
            "target/xtask/openapi.json",
        ]
    );
}

#[test]
fn package_tests_generate_snapshots_only_when_every_producer_runs() {
    let openapi = [BackendSnapshotProfile::OpenApiContract]
        .into_iter()
        .collect();
    let both = [
        BackendSnapshotProfile::OpenApiContract,
        BackendSnapshotProfile::Mysql,
    ]
    .into_iter()
    .collect();
    let api = ["ryframe-api".to_owned()].into_iter().collect();
    let producers = ["ryframe-api".to_owned(), "ryframe-db".to_owned()]
        .into_iter()
        .collect();

    assert!(package_tests_generate_snapshots(&openapi, &api));
    assert!(!package_tests_generate_snapshots(&both, &api));
    assert!(package_tests_generate_snapshots(&both, &producers));
}

#[test]
fn consumer_plan_builds_exact_candidate_and_formal_arguments() {
    let source = serde_json::json!({
        "backend_repository": "Edgar-ycy/ryframe",
        "backend_commit": "1".repeat(40),
    });
    let candidate = consumer_contract_plan(&source, true, Some(&"2".repeat(40))).unwrap();
    assert_eq!(candidate.mode, "candidate");
    assert_eq!(candidate.backend_commit, "2".repeat(40));
    assert_eq!(candidate.backend_repository, "Edgar-ycy/ryframe");
    assert!(!candidate.require_pin);
    assert_eq!(
        consumer_contract_arguments(&candidate, Path::new("target/openapi.json")),
        [
            "consumer:check",
            "--",
            "--mode",
            "candidate",
            "--openapi",
            "target/openapi.json",
            "--backend-commit",
            &"2".repeat(40),
            "--backend-repository",
            "Edgar-ycy/ryframe",
            "--require-pin",
            "false",
        ]
        .map(str::to_owned)
    );

    let formal = consumer_contract_plan(&source, false, None).unwrap();
    assert_eq!(formal.mode, "formal");
    assert_eq!(formal.backend_commit, "1".repeat(40));
    assert!(formal.require_pin);
    let arguments = consumer_contract_arguments(&formal, Path::new("target/openapi.json"));
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--mode", "formal"])
    );
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--require-pin", "true"])
    );
    assert!(consumer_contract_plan(&source, true, None).is_err());
}

#[test]
fn consumer_plan_detects_candidate_marker_from_frontend_workspace() {
    let id = NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "ryframe-xtask-consumer-plan-{}-{id}",
        std::process::id()
    ));
    let openapi = root.join("openapi");
    fs::create_dir_all(&openapi).unwrap();
    fs::write(
        openapi.join("source.json"),
        serde_json::to_vec(&serde_json::json!({
            "backend_repository": "Edgar-ycy/ryframe",
            "backend_commit": "3".repeat(40),
        }))
        .unwrap(),
    )
    .unwrap();

    let formal = load_consumer_contract_plan(&root, None).unwrap();
    assert_eq!(formal.mode, "formal");
    fs::write(openapi.join("candidate.json"), "{}\n").unwrap();
    let candidate = load_consumer_contract_plan(&root, Some(&"4".repeat(40))).unwrap();
    assert_eq!(candidate.mode, "candidate");
    assert_eq!(candidate.backend_commit, "4".repeat(40));
    assert!(!candidate.require_pin);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn full_frontend_commands_do_not_repeat_consumer_owned_gates() {
    let non_consumer = FRONTEND_FULL_NON_CONSUMER_COMMANDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let consumer = CONSUMER_OWNED_COMMANDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        non_consumer.len(),
        FRONTEND_FULL_NON_CONSUMER_COMMANDS.len()
    );
    assert_eq!(consumer.len(), CONSUMER_OWNED_COMMANDS.len());
    assert!(non_consumer.is_disjoint(&consumer));
    assert!(!non_consumer.contains("check"));
    assert_eq!(
        FRONTEND_ONLY_CONTRACT_COMMANDS,
        ["api:check", "typecheck", "test:unit"]
    );
    assert!(!FRONTEND_ONLY_CONTRACT_COMMANDS.contains(&"consumer:check"));
}

#[test]
fn classifies_crate_changes_and_expands_reverse_dependencies() {
    let graph = graph();
    let mut selection = classify_changes(&["crates/kernel/src/lib.rs".into()], &[], &graph);
    assert_eq!(selection.full_reason, None);
    complete_verify_selection(&mut selection, &graph);
    assert_eq!(
        selection.backend_packages,
        [
            "ryframe-api".into(),
            "ryframe-application".into(),
            "ryframe-db".into(),
            "ryframe-kernel".into(),
        ]
        .into_iter()
        .collect()
    );
    assert_eq!(
        selection.backend_snapshot_profiles,
        [
            BackendSnapshotProfile::OpenApiContract,
            BackendSnapshotProfile::Mysql,
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn api_and_database_changes_select_required_backend_snapshots() {
    let graph = graph();

    let mut api = classify_changes(&["crates/api/src/routes.rs".into()], &[], &graph);
    complete_verify_selection(&mut api, &graph);
    assert_eq!(
        api.backend_snapshot_profiles,
        [BackendSnapshotProfile::OpenApiContract]
            .into_iter()
            .collect()
    );
    assert!(needs_consumer_contract(&api.backend_snapshot_profiles));

    let mut database = classify_changes(&["crates/db/src/seeder.rs".into()], &[], &graph);
    complete_verify_selection(&mut database, &graph);
    assert_eq!(
        database.backend_snapshot_profiles,
        [BackendSnapshotProfile::Mysql].into_iter().collect()
    );
    assert!(!needs_consumer_contract(
        &database.backend_snapshot_profiles
    ));
}

#[test]
fn committed_snapshot_changes_are_verified_without_forcing_full_gate() {
    let graph = graph();
    let selection = classify_changes(
        &[
            "openapi/openapi.json".into(),
            "sql/ryframe_config.sql".into(),
        ],
        &[],
        &graph,
    );
    assert_eq!(selection.full_reason, None);
    assert_eq!(
        selection.backend_snapshot_profiles,
        [
            BackendSnapshotProfile::OpenApiContract,
            BackendSnapshotProfile::Mysql,
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn shared_dependency_ci_and_unknown_changes_expand_to_full() {
    let graph = graph();
    for path in ["Cargo.lock", ".github/workflows/ci.yml", "unknown.file"] {
        let selection = classify_changes(&[path.into()], &[], &graph);
        assert!(selection.full_reason.is_some(), "{path} 应扩大为完整门禁");
    }
    let selection = classify_changes(&[], &["pnpm-lock.yaml".into()], &graph);
    assert!(selection.full_reason.is_some());
}

#[test]
fn frontend_code_contract_and_browser_changes_use_separate_profiles() {
    let selection = classify_changes(
        &[],
        &[
            "src/views/post.vue".into(),
            "openapi/openapi.json".into(),
            "tests/browser/post.spec.ts".into(),
        ],
        &graph(),
    );
    assert_eq!(
        selection.frontend_profiles,
        [
            FrontendProfile::Contract,
            FrontendProfile::Code,
            FrontendProfile::Browser,
        ]
        .into_iter()
        .collect()
    );
}

#[test]
fn frontend_contract_plan_runs_api_check_once_and_avoids_owned_duplicates() {
    let contract = [FrontendProfile::Contract].into_iter().collect();
    assert_eq!(
        frontend_profile_commands(&contract, false),
        ["api:check", "typecheck", "test:unit", "build"]
    );

    let contract_and_code = [FrontendProfile::Contract, FrontendProfile::Code]
        .into_iter()
        .collect();
    let commands = frontend_profile_commands(&contract_and_code, false);
    assert_eq!(
        commands
            .iter()
            .filter(|command| **command == "api:check")
            .count(),
        1
    );
    assert!(!commands.contains(&"check:contract"));
    assert!(!commands.contains(&"check:api-artifacts"));
    assert!(!commands.contains(&"check:api-operations"));

    let after_consumer = frontend_profile_commands(&contract_and_code, true);
    assert!(!after_consumer.contains(&"api:check"));
    assert!(!after_consumer.contains(&"typecheck"));
    assert!(!after_consumer.contains(&"test:unit"));
    assert!(after_consumer.contains(&"build"));
}

#[test]
fn documentation_only_changes_need_no_code_profile() {
    let selection = classify_changes(
        &["docs/development.md".into()],
        &["README.md".into(), "ARCHITECTURE.md".into()],
        &WorkspaceGraph {
            package_by_dir: BTreeMap::new(),
            reverse_dependencies: BTreeMap::new(),
        },
    );
    assert_eq!(selection.full_reason, None);
    assert_eq!(selection.backend_packages, BTreeSet::new());
    assert_eq!(selection.frontend_profiles, BTreeSet::new());
}

#[test]
fn markdown_inside_code_or_public_directories_is_not_skipped_as_documentation() {
    let graph = graph();
    let mut selection = classify_changes(
        &["crates/api/src/schema.md".into()],
        &["public/runtime.md".into()],
        &graph,
    );
    complete_verify_selection(&mut selection, &graph);
    assert!(selection.backend_packages.contains("ryframe-api"));
    assert!(
        selection
            .backend_snapshot_profiles
            .contains(&BackendSnapshotProfile::OpenApiContract)
    );
    assert!(selection.frontend_profiles.contains(&FrontendProfile::Code));

    let unknown_docs = classify_changes(&["docs/generated.md".into()], &[], &graph);
    assert!(unknown_docs.full_reason.is_some());
}

#[test]
fn reads_modified_deleted_staged_and_untracked_paths_from_git() {
    let id = NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("ryframe-xtask-git-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "--quiet"]);
    run_git(&root, &["config", "core.autocrlf", "false"]);
    fs::write(root.join("modified.rs"), "before\n").unwrap();
    fs::write(root.join("deleted.rs"), "before\n").unwrap();
    fs::write(root.join("staged.rs"), "before\n").unwrap();
    run_git(&root, &["add", "."]);
    run_git(
        &root,
        &[
            "-c",
            "user.name=RyFrame Test",
            "-c",
            "user.email=ryframe@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "baseline",
        ],
    );
    fs::write(root.join("modified.rs"), "after\n").unwrap();
    fs::remove_file(root.join("deleted.rs")).unwrap();
    fs::write(root.join("staged.rs"), "after\n").unwrap();
    run_git(&root, &["add", "staged.rs"]);
    fs::write(root.join("untracked.rs"), "new\n").unwrap();

    let paths = changed_paths(&root).unwrap();

    assert_eq!(
        paths,
        ["deleted.rs", "modified.rs", "staged.rs", "untracked.rs"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn actual_workspace_graph_contains_reverse_dependents() {
    let graph = load_workspace_graph(&super::workspace::root_dir()).unwrap();
    let closure = reverse_dependency_closure(
        &["ryframe-kernel".to_owned()].into_iter().collect(),
        &graph.reverse_dependencies,
    );
    assert!(closure.contains("ryframe-kernel"));
    assert!(closure.contains("ryframe-application"));
    assert!(closure.contains("ryframe-api"));
    assert!(closure.contains("ryframe"));
}

#[test]
fn change_surface_separates_product_tests_generated_assets_and_tools() {
    let report = analyze_change_surface(
        &[
            "crates/ryframe-application/src/system/user.rs".into(),
            "crates/ryframe-api/src/generated/post.rs".into(),
            "crates/ryframe-db/tests/user_query.rs".into(),
            "crates/ryframe-db/src/migration/m20260824_demo.rs".into(),
            "docs/development.md".into(),
            "xtask/src/check.rs".into(),
        ],
        &[
            "src/views/system/user/index.vue".into(),
            "src/api/generated/operations.ts".into(),
            "tests/unit/user.test.ts".into(),
            "vite.config.ts".into(),
        ],
        &test_change_surface_policy(),
    );

    assert_eq!(report.backend.count(ChangeCategory::HandwrittenProduct), 1);
    assert_eq!(report.backend.count(ChangeCategory::Generated), 1);
    assert_eq!(report.backend.count(ChangeCategory::Test), 1);
    assert_eq!(report.backend.count(ChangeCategory::Migration), 1);
    assert_eq!(report.backend.count(ChangeCategory::Documentation), 1);
    assert_eq!(report.backend.count(ChangeCategory::Tooling), 1);
    assert_eq!(report.frontend.count(ChangeCategory::HandwrittenProduct), 1);
    assert_eq!(report.frontend.count(ChangeCategory::Generated), 1);
    assert_eq!(report.frontend.count(ChangeCategory::Test), 1);
    assert_eq!(report.frontend.count(ChangeCategory::Tooling), 1);
    assert!(report.domains.contains("user"));
}

#[test]
fn change_surface_warns_over_budget_without_blocking_ordinary_changes() {
    let report = analyze_change_surface(
        &[
            "crates/ryframe-api/src/handlers/user_handler.rs".into(),
            "crates/ryframe-api/src/openapi.rs".into(),
        ],
        &[
            "src/views/system/user/index.vue".into(),
            "src/features/user/manifest.ts".into(),
        ],
        &test_change_surface_policy(),
    );

    assert_eq!(report.warnings.len(), 3);
    assert!(report.violations.is_empty());
    assert_eq!(
        report.central_hotspots,
        ["后端:crates/ryframe-api/src/openapi.rs"]
    );
}

#[test]
fn standard_resource_change_rejects_manual_central_registration() {
    let report = analyze_change_surface(
        &[
            "catalog/resources/device.toml".into(),
            "crates/ryframe-api/src/openapi.rs".into(),
        ],
        &[],
        &test_change_surface_policy(),
    );

    assert_eq!(report.violations.len(), 1);
    assert!(report.violations[0].contains("标准资源变更不得手工修改中央热点"));
}

#[test]
fn workspace_change_surface_policy_is_valid_and_versioned() {
    let policy = load_change_surface_policy(&super::workspace::root_dir()).unwrap();
    assert!(!policy.central_hotspots.is_empty());
    assert_eq!(policy.warning_budgets.backend_handwritten_product, 7);
    assert_eq!(policy.soft_source_size.backend_rust, 500);
}

#[test]
fn change_surface_warns_only_for_changed_files_over_soft_size_limits() {
    let id = NEXT_REPOSITORY.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "ryframe-xtask-source-size-{}-{id}",
        std::process::id()
    ));
    let backend = root.join("backend");
    let frontend = root.join("frontend");
    fs::create_dir_all(backend.join("xtask/src")).unwrap();
    fs::create_dir_all(frontend.join("src/views/demo")).unwrap();
    fs::write(backend.join("xtask/src/large.rs"), "line\n".repeat(501)).unwrap();
    fs::write(
        frontend.join("src/views/demo/useLarge.ts"),
        "line\n".repeat(351),
    )
    .unwrap();
    fs::write(
        frontend.join("src/views/demo/ignored.vue"),
        "line\n".repeat(501),
    )
    .unwrap();
    let policy = test_change_surface_policy();
    let backend_paths = ["xtask/src/large.rs".to_owned()];
    let frontend_paths = ["src/views/demo/useLarge.ts".to_owned()];
    let mut report = analyze_change_surface(&backend_paths, &frontend_paths, &policy);
    append_changed_file_size_warnings(
        &backend,
        &frontend,
        &backend_paths,
        &frontend_paths,
        &policy,
        &mut report,
    )
    .unwrap();
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("large.rs"))
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.contains("useLarge.ts"))
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|warning| warning.contains("ignored.vue"))
    );
    fs::remove_dir_all(root).unwrap();
}

fn test_change_surface_policy() -> ChangeSurfacePolicy {
    toml::from_str(
        r#"
version = 1

[warning_budgets]
backend_handwritten_product = 1
frontend_handwritten_product = 1
combined_handwritten_product = 1

[soft_source_size]
backend_rust = 500
frontend_composable = 350
frontend_sfc_or_style = 500

[[central_hotspots]]
repository = "backend"
path = "crates/ryframe-api/src/openapi.rs"
standard_resource_forbidden = true
"#,
    )
    .unwrap()
}

fn run_git(root: &std::path::Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
}
