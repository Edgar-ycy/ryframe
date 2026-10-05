use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs,
    path::Path,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use super::check::{
    BACKEND_CI_TARGET_DIR, BACKEND_SMART_TARGET_DIR, BACKEND_VERIFY_TARGET_DIR,
    BackendSnapshotProfile, ChangeCategory, ChangeSurfacePolicy, CheckPlanMode, FrontendProfile,
    PYTHON_ENVIRONMENT_ARGS, PYTHON_TEST_ARGS, PolicyProfile, RESOURCE_CI_TARGET_DIR,
    RESOURCE_VERIFY_TARGET_DIR, RepositoryKind, ResourceWorkspaceProfile, SMART_BACKEND_OPERATIONS,
    SMART_FEATURE_OPERATIONS, VerifyTargetPolicy, WORKSPACE_CLIPPY_ARGS, WorkspaceGraph,
    analyze_change_surface, append_changed_file_size_warnings, backend_package_operation_args,
    cargo_operation_jobs, changed_paths, ci_environment_from, ci_target_policy_from,
    ci_test_jobs_from, classify_changes, complete_verify_selection, consumer_contract_arguments,
    consumer_contract_command, consumer_contract_plan, default_test_jobs_from,
    feature_operation_args, feature_test_args, frontend_profile_commands,
    load_change_surface_policy, load_consumer_contract_plan, load_workspace_graph,
    minimal_workspace_check_args, needs_consumer_contract, package_tests_generate_snapshots,
    parse_change_surface_policy, policy_tasks, resolve_frontend_dir, resolve_target_dir,
    resource_test_executable_from_messages, resource_workspace_environment_for_profile,
    reverse_dependency_closure, select_check_mode, validate_feature_combination,
    verify_job_budget_from, verify_target_policy_from, workspace_clippy_args, workspace_test_args,
};
use super::cli::CheckScope;

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
    let features = vec!["bin-reset".to_owned(), "bin-file-maintenance".to_owned()];
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
        assert!(args.contains(&"bin-reset,bin-file-maintenance".to_owned()));
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
    assert!(test.contains(&"bin-reset,bin-file-maintenance".to_owned()));
    assert!(clippy.ends_with(&[
        "--".into(),
        "-D".into(),
        "warnings".into(),
        "-D".into(),
        "clippy::redundant_clone".into(),
    ]));
}

#[test]
fn full_gate_discovers_repository_python_and_node_tests() {
    assert_eq!(
        PYTHON_ENVIRONMENT_ARGS,
        ["tools/python/check_python_environment.py"]
    );
    assert_eq!(
        PYTHON_TEST_ARGS,
        [
            "-m",
            "unittest",
            "discover",
            "-s",
            "tools/python",
            "-p",
            "test_*.py",
        ]
    );
    assert_eq!(
        policy_tasks(PolicyProfile::FullStatic)
            .into_iter()
            .map(|task| task.script)
            .collect::<Vec<_>>(),
        [
            "tools/python/check_architecture.py",
            "tools/python/check_deployment_assets.py",
            "tools/python/check_prerelease_dependencies.py",
            "tools/python/check_permission_routes.py",
            "tools/python/check_removed_identity.py",
            "tools/python/check_supply_chain.py",
        ]
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
fn devex_ci_target_override_isolated_backend_and_resource_outputs() {
    let root = Path::new("D:/devex/sample/cache");
    assert_eq!(
        ci_target_policy_from(Some(root)),
        VerifyTargetPolicy {
            backend: root.join("backend").to_string_lossy().into_owned(),
            resource: root.join("resource").to_string_lossy().into_owned(),
        }
    );
    assert_eq!(
        ci_target_policy_from(None),
        VerifyTargetPolicy {
            backend: BACKEND_CI_TARGET_DIR.to_owned(),
            resource: RESOURCE_CI_TARGET_DIR.to_owned(),
        }
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
fn resource_workspace_environment_uses_selected_frontend_and_target() {
    assert_eq!(
        resource_workspace_environment_for_profile(
            Path::new("workspace/frontend"),
            Path::new("target/ci/resource"),
            "4",
            ResourceWorkspaceProfile::Full,
        ),
        [
            ("CARGO_BUILD_JOBS", "4".to_owned()),
            (
                "RYFRAME_RESOURCE_WORKSPACE_FRONTEND_DIR",
                "workspace/frontend".to_owned(),
            ),
            (
                "RYFRAME_RESOURCE_WORKSPACE_TARGET_DIR",
                "target/ci/resource".to_owned(),
            ),
        ]
    );
}

#[test]
fn only_targeted_resource_workspace_sets_the_lightweight_profile() {
    let full = resource_workspace_environment_for_profile(
        Path::new("workspace/frontend"),
        Path::new("target/ci/resource"),
        "4",
        ResourceWorkspaceProfile::Full,
    );
    assert!(
        full.iter()
            .all(|(key, _)| *key != "RYFRAME_RESOURCE_WORKSPACE_PROFILE")
    );

    let targeted = resource_workspace_environment_for_profile(
        Path::new("workspace/frontend"),
        Path::new("target/ci/resource"),
        "4",
        ResourceWorkspaceProfile::Targeted,
    );
    assert!(targeted.contains(&("RYFRAME_RESOURCE_WORKSPACE_PROFILE", "targeted".to_owned(),)));
}

#[test]
fn resource_frontend_resolution_is_absolute_and_child_process_safe() {
    let workspace = std::env::current_dir().unwrap();
    let resolved = resolve_frontend_dir(&workspace.join("xtask"), Path::new("..")).unwrap();

    assert!(resolved.is_absolute());
    #[cfg(windows)]
    assert!(
        !resolved.to_string_lossy().starts_with(r"\\?\"),
        "传给 Node 的前端路径不得使用 Windows verbatim 前缀：{}",
        resolved.display()
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
    assert_eq!(
        consumer_contract_command(false),
        ["check", "--stage", "contract"]
    );
    assert_eq!(consumer_contract_command(true), ["check", "--full"]);

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
fn shared_check_plan_respects_explicit_full_and_selected_scope() {
    let explicit = select_check_mode(
        CheckScope::All,
        true,
        &["unknown-backend.file".into()],
        &["unknown-frontend.file".into()],
        &graph(),
    );
    assert_eq!(explicit, CheckPlanMode::ExplicitFull);

    let selected = select_check_mode(
        CheckScope::Frontend,
        false,
        &["Cargo.lock".into()],
        &["src/views/post.vue".into()],
        &graph(),
    );
    let CheckPlanMode::Selected(selection) = selected else {
        panic!("前端单侧计划不应被未选择的后端变更扩大");
    };
    assert!(selection.backend_packages.is_empty());
    assert_eq!(
        selection.frontend_profiles,
        [FrontendProfile::Code].into_iter().collect()
    );
}

#[test]
fn shared_check_plan_preserves_full_expansion_reason() {
    let mode = select_check_mode(
        CheckScope::All,
        false,
        &["unknown.file".into()],
        &[],
        &graph(),
    );
    let CheckPlanMode::ExpandedFull(reason) = mode else {
        panic!("未知变更必须扩大为完整门禁");
    };
    assert_eq!(reason, "无法安全分类后端变更：unknown.file");
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
fn frontend_contract_profiles_select_public_stages_and_one_unit_invocation() {
    let contract = [FrontendProfile::Contract].into_iter().collect();
    assert_eq!(
        frontend_profile_commands(&contract, false),
        [
            ["check", "--stage", "contract"].as_slice(),
            ["check", "--stage", "unit"].as_slice(),
            ["build"].as_slice(),
        ]
    );

    let contract_and_code = [FrontendProfile::Contract, FrontendProfile::Code]
        .into_iter()
        .collect();
    let commands = frontend_profile_commands(&contract_and_code, false);
    assert_eq!(
        commands,
        [
            ["check", "--stage", "static"].as_slice(),
            ["check", "--stage", "unit"].as_slice(),
            ["build"].as_slice(),
        ]
    );

    let after_consumer = frontend_profile_commands(&contract_and_code, true);
    assert_eq!(
        after_consumer,
        [
            ["check", "--stage", "static"].as_slice(),
            ["build"].as_slice(),
        ]
    );
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
fn resource_workspace_graph_excludes_optional_tooling_dependents() {
    let graph = super::check::load_resource_workspace_graph(&super::workspace::root_dir()).unwrap();
    let closure = reverse_dependency_closure(
        &["ryframe-application".to_owned()].into_iter().collect(),
        &graph.reverse_dependencies,
    );

    assert!(closure.contains("ryframe-application"));
    assert!(closure.contains("ryframe-api"));
    assert!(!closure.contains("ryframe-generator"));
    assert!(!closure.contains("xtask"));
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
            "catalog/resources/post.toml".into(),
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
    assert_eq!(policy.soft_source_size.warning_percent, 80);
    assert_eq!(policy.soft_source_size.attention_percent, 90);
    assert_eq!(policy.soft_source_size.backend_rust_hard_limit, 600);
    assert_eq!(policy.soft_source_size.frontend_sfc_hard_limit, 400);
    assert!(
        policy
            .full_invalidation_reason(RepositoryKind::Backend, "Cargo.toml")
            .is_some()
    );
    assert!(
        policy
            .full_invalidation_reason(RepositoryKind::Backend, ".cargo/config.toml")
            .is_some()
    );
    assert!(
        policy
            .full_invalidation_reason(RepositoryKind::Backend, "catalog/resources/post.toml")
            .is_none()
    );
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
    fs::write(backend.join("xtask/src/large.rs"), "line\n".repeat(480)).unwrap();
    fs::write(backend.join("xtask/src/small.rs"), "line\n".repeat(479)).unwrap();
    fs::write(
        frontend.join("src/views/demo/useLarge.ts"),
        "line\n".repeat(270),
    )
    .unwrap();
    fs::write(
        frontend.join("src/views/demo/ignored.vue"),
        "line\n".repeat(501),
    )
    .unwrap();
    let policy = test_change_surface_policy();
    let backend_paths = [
        "xtask/src/large.rs".to_owned(),
        "xtask/src/small.rs".to_owned(),
    ];
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
    assert_eq!(report.warnings.len(), 2);
    let warnings = report.warnings.join("\n");
    assert!(warnings.contains("large.rs") && warnings.contains("80%"));
    assert!(warnings.contains("useLarge.ts") && warnings.contains("90%"));
    assert!(!warnings.contains("small.rs"));
    assert!(!warnings.contains("ignored.vue"));
    fs::remove_dir_all(root).unwrap();
}

fn test_change_surface_policy() -> ChangeSurfacePolicy {
    parse_change_surface_policy(&test_change_surface_policy_source()).unwrap()
}

fn test_change_surface_policy_source() -> String {
    include_str!("../fixtures/change_surface_policy.toml").to_owned()
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
