#![allow(dead_code)]

use std::error::Error;

#[path = "../src/check.rs"]
mod check;
#[path = "../src/cli.rs"]
mod cli;
#[path = "../src/contract.rs"]
mod contract;
#[path = "../src/dev.rs"]
mod dev;
#[path = "../src/diff.rs"]
mod diff;
#[path = "../src/doctor.rs"]
mod doctor;
#[path = "../src/migration.rs"]
mod migration;
#[path = "../src/process.rs"]
mod process;
#[path = "../src/release.rs"]
mod release;
#[path = "../src/resource.rs"]
mod resource;
#[path = "../src/watch.rs"]
mod watch;
#[path = "../src/workspace.rs"]
mod workspace;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

mod dev_tests {
    use std::{
        cell::RefCell,
        path::Path,
        time::{Duration, Instant},
    };

    use super::dev::{
        api_command, available_ports, combine_failures, start_worker_after_api_ready,
        switch_services_with_rollback, wait_services_ready_until, worker_command,
    };

    #[test]
    fn worker_probe_uses_isolated_non_consuming_mode() {
        let command = worker_command(
            Path::new("D:/workspace"),
            Path::new("D:/workspace/worker.exe"),
            19091,
            4,
            true,
        );
        assert_eq!(
            command
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["--probe"]
        );
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            environment.get("APP_JOBS_WORKER_ID"),
            Some(&Some("ryframe-dev-probe".into()))
        );
        assert_eq!(
            environment.get("APP_JOBS_HEALTH_PORT"),
            Some(&Some("19091".into()))
        );
        assert_eq!(
            environment.get("APP_JOBS_HEALTH_HOST"),
            Some(&Some("127.0.0.1".into()))
        );
        assert_eq!(
            environment.get("APP_TELEMETRY_ENABLED"),
            Some(&Some("false".into()))
        );
        assert_eq!(
            environment.get("APP_LOGGER_OUTPUT"),
            Some(&Some("stdout".into()))
        );
    }

    #[test]
    fn api_probe_uses_explicit_side_effect_free_mode() {
        let command = api_command(
            Path::new("D:/workspace"),
            Path::new("D:/workspace/api.exe"),
            18080,
            19091,
            3,
            true,
        );
        assert_eq!(
            command
                .get_args()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["--probe"]
        );
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            environment.get("APP_APP_HOST"),
            Some(&Some("127.0.0.1".into()))
        );
        assert_eq!(
            environment.get("APP_TELEMETRY_ENABLED"),
            Some(&Some("false".into()))
        );
        assert_eq!(
            environment.get("APP_LOGGER_OUTPUT"),
            Some(&Some("stdout".into()))
        );
    }

    #[test]
    fn candidate_probe_reserves_distinct_api_and_worker_ports() {
        let (api, worker) = available_ports().unwrap();
        assert_ne!(api, worker);
    }

    #[test]
    fn formal_worker_starts_only_after_api_is_ready() {
        let calls = RefCell::new(Vec::new());
        let worker = start_worker_after_api_ready(
            || {
                calls.borrow_mut().push("ready:api");
                Ok(())
            },
            || {
                calls.borrow_mut().push("start:worker");
                Ok("worker")
            },
        )
        .unwrap();

        assert_eq!(worker, "worker");
        assert_eq!(calls.into_inner(), ["ready:api", "start:worker"]);
        let error = start_worker_after_api_ready(
            || Err("api readiness failed".into()),
            || -> super::Result<()> { panic!("API 未就绪时不能启动 Worker") },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("api readiness failed"));
    }

    #[test]
    fn services_use_one_expired_health_deadline() {
        let running_checks = RefCell::new(Vec::new());
        let error = wait_services_ready_until(
            Instant::now() - Duration::from_secs(1),
            "API",
            "Worker",
            || {
                running_checks.borrow_mut().push("running:api");
                Ok(())
            },
            || {
                running_checks.borrow_mut().push("running:worker");
                Ok(())
            },
            || panic!("截止时间已过时不能继续探活 API"),
            || panic!("截止时间已过时不能继续探活 Worker"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("共享 30 秒截止时间"));
        assert_eq!(
            running_checks.into_inner(),
            ["running:api", "running:worker"]
        );
    }

    #[test]
    fn services_report_process_exit_before_readiness_work() {
        let error = wait_services_ready_until(
            Instant::now() + Duration::from_secs(1),
            "API",
            "Worker",
            || Err("api exited".into()),
            || panic!("API 已退出时不应继续检查 Worker"),
            || panic!("API 已退出时不应请求 API readyz"),
            || panic!("API 已退出时不应请求 Worker readyz"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("api exited"));
    }

    #[test]
    fn normal_services_start_without_probe_mode() {
        let api = api_command(
            Path::new("D:/workspace"),
            Path::new("D:/workspace/api.exe"),
            18080,
            19091,
            3,
            false,
        );
        let worker = worker_command(
            Path::new("D:/workspace"),
            Path::new("D:/workspace/worker.exe"),
            19091,
            4,
            false,
        );

        assert_eq!(api.get_args().count(), 0);
        assert_eq!(worker.get_args().count(), 0);
        let api_environment = api
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let worker_environment = worker
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            worker_environment.get("APP_JOBS_WORKER_ID"),
            Some(&Some("ryframe-dev-worker".into()))
        );
        assert!(!worker_environment.contains_key("APP_JOBS_HEALTH_HOST"));
        for environment in [&api_environment, &worker_environment] {
            assert!(!environment.contains_key("APP_TELEMETRY_ENABLED"));
            assert!(!environment.contains_key("APP_LOGGER_OUTPUT"));
        }
    }

    #[test]
    fn probe_reports_health_and_cleanup_failures_together() {
        let error = combine_failures(
            Err::<(), _>("候选健康失败".into()),
            [
                ("停止候选 API", Err("API 清理失败".into())),
                ("停止候选 Worker", Err("Worker 清理失败".into())),
            ],
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("候选健康失败"));
        assert!(error.contains("停止候选 API 失败：API 清理失败"));
        assert!(error.contains("停止候选 Worker 失败：Worker 清理失败"));
    }

    #[test]
    fn candidate_start_failure_restores_last_known_good() {
        #[derive(Debug, PartialEq)]
        struct FakeServices {
            version: &'static str,
            running: bool,
        }

        let calls = RefCell::new(Vec::new());
        let mut current = FakeServices {
            version: "previous",
            running: true,
        };
        let promoted = switch_services_with_rollback(
            &mut current,
            |services| {
                calls.borrow_mut().push("stop:previous");
                services.running = false;
                Ok(())
            },
            || {
                calls.borrow_mut().push("start:candidate");
                Err("candidate formal start failed".into())
            },
            || {
                calls.borrow_mut().push("start:previous");
                Ok(FakeServices {
                    version: "previous",
                    running: true,
                })
            },
            || calls.borrow_mut().push("cleanup:previous"),
        )
        .unwrap();

        assert!(!promoted);
        assert_eq!(
            current,
            FakeServices {
                version: "previous",
                running: true,
            }
        );
        assert_eq!(
            calls.into_inner(),
            ["stop:previous", "start:candidate", "start:previous"]
        );
    }

    #[test]
    fn candidate_and_restore_failures_are_reported_together() {
        let mut current = "previous";
        let error = switch_services_with_rollback(
            &mut current,
            |_| Ok(()),
            || Err("candidate formal start failed".into()),
            || Err("previous restore failed".into()),
            || panic!("候选启动失败时不能清理 last-known-good"),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("candidate formal start failed"));
        assert!(error.contains("previous restore failed"));
    }

    #[test]
    fn candidate_start_success_cleans_previous_after_start() {
        let calls = RefCell::new(Vec::new());
        let mut current = "previous";
        let promoted = switch_services_with_rollback(
            &mut current,
            |_| {
                calls.borrow_mut().push("stop:previous");
                Ok(())
            },
            || {
                calls.borrow_mut().push("start:candidate");
                Ok("candidate")
            },
            || panic!("候选启动成功时不能恢复 last-known-good"),
            || calls.borrow_mut().push("cleanup:previous"),
        )
        .unwrap();

        assert!(promoted);
        assert_eq!(current, "candidate");
        assert_eq!(
            calls.into_inner(),
            ["stop:previous", "start:candidate", "cleanup:previous"]
        );
    }
}

mod diff_tests {
    use super::diff::unified;

    #[test]
    fn unchanged_content_has_no_diff() {
        assert!(unified("same\n", "same\n", "generated/a.rs").is_empty());
    }

    #[test]
    fn replacement_keeps_context_and_labels() {
        let diff = unified(
            "one\ntwo\nthree\nfour\nfive\n",
            "one\ntwo\nchanged\nfour\nfive\n",
            "generated/a.rs",
        );
        assert!(diff.contains("--- 工作区/generated/a.rs"));
        assert!(diff.contains("+++ 生成结果/generated/a.rs"));
        assert!(diff.contains("-three\n+changed"));
        assert!(diff.contains(" two"));
    }

    #[test]
    fn new_file_uses_zero_length_old_hunk() {
        let diff = unified("", "first\nsecond\n", "generated/new.rs");
        assert!(diff.contains("@@ -0,0 +1,2 @@"));
        assert!(diff.contains("+first\n+second"));
    }

    #[test]
    fn missing_terminal_newline_is_visible() {
        let diff = unified("value", "value\n", "generated/a.rs");
        assert!(diff.contains("\\ No newline at end of file"));
    }
}

mod check_tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
        path::Path,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::check::{
        BACKEND_POLICY_SCRIPTS, BackendSnapshotProfile, CONSUMER_OWNED_COMMANDS,
        FRONTEND_FULL_NON_CONSUMER_COMMANDS, FRONTEND_ONLY_CONTRACT_COMMANDS, FrontendProfile,
        PYTHON_TEST_ARGS, RESOURCE_WORKSPACE_TEST_TARGET_DIR, WORKSPACE_CLIPPY_ARGS,
        WORKSPACE_TEST_ARGS, WORKSPACE_TEST_TARGET_DIR, WorkspaceGraph, changed_paths,
        classify_changes, complete_verify_selection, consumer_contract_arguments,
        consumer_contract_plan, feature_operation_args, feature_test_args,
        frontend_profile_commands, load_consumer_contract_plan, load_workspace_graph,
        needs_consumer_contract, reverse_dependency_closure, validate_feature_combination,
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
        let check = feature_operation_args("check", "ryframe", &features);
        let clippy = feature_operation_args("clippy", "ryframe", &features);

        for args in [&check, &clippy] {
            assert!(args.windows(2).any(|pair| pair == ["-p", "ryframe"]));
            assert!(
                args.windows(2)
                    .any(|pair| { pair == ["--target-dir", "target/feature-matrix"] })
            );
            assert!(args.contains(&"--all-targets".to_owned()));
            assert!(args.contains(&"--no-default-features".to_owned()));
            assert!(args.contains(&"destructive-reset,file-maintenance".to_owned()));
        }
        let test = feature_test_args("ryframe", &features, "reset_contract");
        assert!(test.windows(2).any(|pair| pair == ["-p", "ryframe"]));
        assert!(test.windows(2).any(|pair| pair == ["--jobs", "2"]));
        assert!(
            test.windows(2)
                .any(|pair| { pair == ["--target-dir", "target/feature-matrix"] })
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
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
                "-D",
                "clippy::redundant_clone",
            ]
        );
        assert_eq!(
            WORKSPACE_TEST_ARGS,
            [
                "test",
                "--locked",
                "--target-dir",
                "target/workspace-tests",
                "--workspace",
                "--jobs",
                "2",
            ]
        );
        assert_eq!(WORKSPACE_TEST_TARGET_DIR, "target/workspace-tests");
        assert_eq!(
            RESOURCE_WORKSPACE_TEST_TARGET_DIR,
            "target/resource-workspace-tests"
        );
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
        let root =
            std::env::temp_dir().join(format!("ryframe-xtask-git-{}-{id}", std::process::id()));
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
}

mod cli_tests {
    use std::path::PathBuf;

    use super::cli::{
        ApiSyncCommand, CheckScope, CliError, Command, ContractOperation, MigrationCommand,
        MigrationOperation, MigrationTarget, ResourceAction, parse,
    };

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    fn parse_command(values: &[&str]) -> std::result::Result<Command, CliError> {
        parse(strings(values)).map(|cli| cli.command)
    }

    #[test]
    fn preserves_read_only_legacy_check_commands() {
        assert_eq!(
            parse_command(&["check", "--scope", "backend"]).unwrap(),
            Command::Check {
                scope: CheckScope::Backend
            }
        );
        assert_eq!(
            parse_command(&["contract", "check"]).unwrap(),
            Command::Contract {
                operation: ContractOperation::Check
            }
        );
        assert!(parse_command(&["contract", "sync"]).is_err());
    }

    #[test]
    fn parses_daily_short_commands() {
        assert_eq!(
            parse_command(&["verify", "--full", "--scope", "frontend"]).unwrap(),
            Command::Verify {
                scope: CheckScope::Frontend,
                full: true
            }
        );
        let Command::Resource(resource) =
            parse_command(&["resource", "post", "--explain"]).unwrap()
        else {
            panic!("应解析为资源命令");
        };
        assert_eq!(resource.name, "post");
        assert_eq!(resource.action, ResourceAction::Explain);
        assert_eq!(
            parse_command(&["api-sync", "--commit", "HEAD"]).unwrap(),
            Command::ApiSync(ApiSyncCommand::Commit("HEAD".into()))
        );
        assert_eq!(
            parse_command(&["migrate", "freeze"]).unwrap(),
            Command::Migrate(MigrationCommand::Freeze)
        );
    }

    #[test]
    fn migration_defaults_to_control_scope() {
        assert_eq!(
            parse_command(&["migrate", "verify"]).unwrap(),
            Command::Migrate(MigrationCommand::Run {
                operation: MigrationOperation::Verify,
                target: MigrationTarget::Control,
            })
        );
        assert_eq!(
            parse_command(&["migrate", "up", "tenant-data", "--all"]).unwrap(),
            Command::Migrate(MigrationCommand::Run {
                operation: MigrationOperation::Up,
                target: MigrationTarget::TenantDataAll,
            })
        );
    }

    #[test]
    fn global_frontend_dir_can_follow_command_arguments() {
        let cli = parse(strings(&[
            "contract",
            "check",
            "--frontend-dir",
            "D:/workspace/frontend",
        ]))
        .unwrap();
        assert_eq!(cli.frontend_dir, PathBuf::from("D:/workspace/frontend"));
    }

    #[test]
    fn rejects_ambiguous_or_duplicate_arguments() {
        assert!(parse_command(&["resource", "post", "--write", "--explain"]).is_err());
        assert!(parse_command(&["verify", "--scope", "all", "--scope", "backend"]).is_err());
        assert!(parse_command(&["migrate", "new", "unknown", "add_device"]).is_err());
        assert!(parse(strings(&["verify", "--frontend-dir", "--full"])).is_err());
        assert!(parse_command(&["api-sync", "--commit", "--full"]).is_err());
        assert!(parse_command(&["api-sync", "--commit", "-q"]).is_err());
        assert!(parse_command(&["migrate", "verify", "tenant-data", "--target", "--all"]).is_err());
    }

    #[test]
    fn daily_commands_forward_every_supported_argument() {
        assert_eq!(
            parse_command(&["verify", "--scope", "backend", "--full"]).unwrap(),
            Command::Verify {
                scope: CheckScope::Backend,
                full: true,
            }
        );
        assert!(matches!(
            parse_command(&["resource", "post", "--write"]).unwrap(),
            Command::Resource(command) if command.name == "post" && command.action == ResourceAction::Write
        ));
        assert_eq!(
            parse_command(&["migrate", "status", "tenant-data", "--target", "tenant-a"]).unwrap(),
            Command::Migrate(MigrationCommand::Run {
                operation: MigrationOperation::Status,
                target: MigrationTarget::TenantDataOne("tenant-a".into()),
            })
        );
    }
}

mod contract_tests {
    use std::{
        fs, io,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::contract::{
        ContractFileOperations, Snapshot, apply_candidate, apply_candidate_with_staging_hook,
        generated_artifact_paths, github_repository_identifier, install_snapshots_with, sha256_hex,
        validate_candidate_contract, validate_formal_sync, write_atomically_with,
    };

    const CANDIDATE_MANAGED_PATHS: &[&str] = &[
        "openapi/openapi.json",
        "src/api/generated/schema.ts",
        "src/api/generated/operations.ts",
        "src/api/generated/permissions.ts",
        "src/api/generated/menuRoutes.ts",
        "src/shared/security/passwordPolicy.generated.json",
        "src/shared/markdown/noticePolicy.generated.json",
        "src/shared/config/apiPrefix.generated.json",
        "src/api/generated/crudResources.ts",
    ];

    static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

    struct TestFrontend(PathBuf);

    impl TestFrontend {
        fn new() -> Self {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "ryframe-xtask-contract-{}-{id}",
                std::process::id()
            ));
            for relative in CANDIDATE_MANAGED_PATHS {
                let path = root.join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, format!("original:{relative}")).unwrap();
            }
            let manifest = root.join("scripts/api-artifacts.mjs");
            fs::create_dir_all(manifest.parent().unwrap()).unwrap();
            let entries = CANDIDATE_MANAGED_PATHS[1..]
                .iter()
                .map(|path| format!("  '{path}',"))
                .collect::<Vec<_>>()
                .join("\n");
            fs::write(
                manifest,
                format!("export const generatedArtifactPaths = Object.freeze([\n{entries}\n])\n"),
            )
            .unwrap();
            fs::write(root.join("openapi/source.json"), "formal-source").unwrap();
            let backend_openapi = root.join("backend/openapi/openapi.json");
            fs::create_dir_all(backend_openapi.parent().unwrap()).unwrap();
            fs::write(&backend_openapi, "original-backend-openapi").unwrap();
            Self(root)
        }

        fn backend(&self) -> PathBuf {
            self.0.join("backend")
        }
    }

    impl Drop for TestFrontend {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn candidate() -> &'static [u8] {
        b"{\n  \"info\": {\n    \"title\": \"RyFrame API\"\n  },\n  \"openapi\": \"3.1.0\"\n}\n"
    }

    struct AtomicFaults {
        fail_backup_cleanup: bool,
        fail_install: bool,
        fail_restore: bool,
        target: PathBuf,
    }

    impl ContractFileOperations for AtomicFaults {
        fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
            let name = source
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if target == self.target
                && ((self.fail_install && name.contains(".xtask-new-"))
                    || (self.fail_restore && name.contains(".xtask-backup-")))
            {
                return Err(io::Error::other("故障注入 rename"));
            }
            fs::rename(source, target)
        }

        fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
            fs::hard_link(source, target)
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if self.fail_backup_cleanup && name.contains(".xtask-backup-") {
                return Err(io::Error::other("故障注入 backup cleanup"));
            }
            fs::remove_file(path)
        }
    }

    struct InstallFaults {
        hard_link_calls: AtomicU64,
        fail_at: u64,
        edit_after: Option<(u64, PathBuf)>,
    }

    impl ContractFileOperations for InstallFaults {
        fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
            fs::rename(source, target)
        }

        fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
            let call = self.hard_link_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_at {
                return Err(io::Error::other("注入第 N 个契约文件安装失败"));
            }
            fs::hard_link(source, target)?;
            if self
                .edit_after
                .as_ref()
                .is_some_and(|(at, path)| *at == call && path == target)
            {
                fs::remove_file(target)?;
                fs::write(target, b"manual-after-install")?;
            }
            Ok(())
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            fs::remove_file(path)
        }
    }

    fn backup_files(parent: &Path) -> Vec<PathBuf> {
        fs::read_dir(parent)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".xtask-backup-"))
            })
            .collect()
    }

    #[test]
    fn normalizes_github_repository_identifiers() {
        assert_eq!(
            github_repository_identifier("https://github.com/Edgar-ycy/ryframe.git").unwrap(),
            "Edgar-ycy/ryframe"
        );
        assert_eq!(
            github_repository_identifier("git@github.com:Edgar-ycy/ryframe.git").unwrap(),
            "Edgar-ycy/ryframe"
        );
        assert!(github_repository_identifier("https://example.com/demo/repo").is_err());
    }

    #[test]
    fn candidate_sync_keeps_formal_source_metadata() {
        let frontend = TestFrontend::new();
        apply_candidate(&frontend.backend(), &frontend.0, candidate(), |_| Ok(())).unwrap();
        assert_eq!(
            fs::read(frontend.0.join("openapi/openapi.json")).unwrap(),
            candidate()
        );
        assert_eq!(
            fs::read_to_string(frontend.0.join("openapi/source.json")).unwrap(),
            "formal-source"
        );
        assert_eq!(
            fs::read(frontend.backend().join("openapi/openapi.json")).unwrap(),
            candidate()
        );
        assert!(frontend.0.join("openapi/candidate.json").is_file());
    }

    #[test]
    fn candidate_sync_rejects_changed_generation_inputs_before_install() {
        let frontend = TestFrontend::new();
        let manifest = frontend.0.join("scripts/api-artifacts.mjs");
        let result = apply_candidate_with_staging_hook(
            &frontend.backend(),
            &frontend.0,
            candidate(),
            |_| {
                fs::write(&manifest, "export const generatedArtifactPaths = []\n")?;
                Ok(())
            },
            |_| Ok(()),
        );

        let error = result
            .expect_err("生成输入变化时必须在安装前失败")
            .to_string();
        assert!(error.contains("契约生成输入在事务期间发生变化"));
        assert_eq!(
            fs::read_to_string(frontend.0.join("openapi/openapi.json")).unwrap(),
            "original:openapi/openapi.json"
        );
        assert_eq!(
            fs::read_to_string(frontend.backend().join("openapi/openapi.json")).unwrap(),
            "original-backend-openapi"
        );
        assert!(!frontend.0.join("openapi/candidate.json").exists());
    }

    #[test]
    fn multi_file_install_failure_restores_every_contract_file() {
        let frontend = TestFrontend::new();
        let paths = [
            frontend.0.join("openapi/candidate.json"),
            frontend.0.join("src/api/generated/schema.ts"),
            frontend.0.join("src/api/generated/menuRoutes.ts"),
        ];
        for (index, path) in paths.iter().enumerate() {
            fs::write(path, format!("before-{index}")).unwrap();
        }
        let before = paths
            .iter()
            .enumerate()
            .map(|(index, path)| Snapshot {
                path: path.clone(),
                content: Some(format!("before-{index}").into_bytes()),
            })
            .collect::<Vec<_>>();
        let desired = paths
            .iter()
            .enumerate()
            .map(|(index, path)| Snapshot {
                path: path.clone(),
                content: Some(format!("after-{index}").into_bytes()),
            })
            .collect::<Vec<_>>();
        let operations = InstallFaults {
            hard_link_calls: AtomicU64::new(0),
            fail_at: 2,
            edit_after: None,
        };

        let error = install_snapshots_with(&before, &desired, &operations)
            .expect_err("第二个文件安装失败必须回滚")
            .to_string();

        assert!(error.contains("第 N 个契约文件安装失败"));
        for (index, path) in paths.iter().enumerate() {
            assert_eq!(fs::read_to_string(path).unwrap(), format!("before-{index}"));
        }
        assert!(
            paths
                .iter()
                .flat_map(|path| fs::read_dir(path.parent().unwrap()).unwrap())
                .filter_map(std::result::Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().contains(".xtask-"))
        );
    }

    #[test]
    fn rollback_preserves_manual_edit_after_contract_install() {
        let frontend = TestFrontend::new();
        let paths = [
            frontend.0.join("openapi/candidate.json"),
            frontend.0.join("src/api/generated/schema.ts"),
            frontend.0.join("src/api/generated/menuRoutes.ts"),
        ];
        for (index, path) in paths.iter().enumerate() {
            fs::write(path, format!("before-{index}")).unwrap();
        }
        let before = paths
            .iter()
            .enumerate()
            .map(|(index, path)| Snapshot {
                path: path.clone(),
                content: Some(format!("before-{index}").into_bytes()),
            })
            .collect::<Vec<_>>();
        let desired = paths
            .iter()
            .enumerate()
            .map(|(index, path)| Snapshot {
                path: path.clone(),
                content: Some(format!("after-{index}").into_bytes()),
            })
            .collect::<Vec<_>>();
        let operations = InstallFaults {
            hard_link_calls: AtomicU64::new(0),
            fail_at: 2,
            edit_after: Some((1, paths[0].clone())),
        };

        let error = install_snapshots_with(&before, &desired, &operations)
            .expect_err("并发编辑后续失败必须保留恢复状态")
            .to_string();

        assert!(error.contains("再次修改"));
        assert_eq!(
            fs::read_to_string(&paths[0]).unwrap(),
            "manual-after-install"
        );
        assert!(backup_files(paths[0].parent().unwrap()).len() == 1);
        assert!(
            fs::read_dir(paths[0].parent().unwrap())
                .unwrap()
                .filter_map(std::result::Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".xtask-contract-transaction-"))
        );
    }

    #[test]
    fn contract_success_path_rechecks_targets_before_backup_cleanup() {
        let frontend = TestFrontend::new();
        let paths = [
            frontend.0.join("openapi/candidate.json"),
            frontend.0.join("src/api/generated/schema.ts"),
        ];
        for (index, path) in paths.iter().enumerate() {
            fs::write(path, format!("before-{index}")).unwrap();
        }
        let before = paths
            .iter()
            .enumerate()
            .map(|(index, path)| Snapshot {
                path: path.clone(),
                content: Some(format!("before-{index}").into_bytes()),
            })
            .collect::<Vec<_>>();
        let desired = paths
            .iter()
            .enumerate()
            .map(|(index, path)| Snapshot {
                path: path.clone(),
                content: Some(format!("after-{index}").into_bytes()),
            })
            .collect::<Vec<_>>();
        let operations = InstallFaults {
            hard_link_calls: AtomicU64::new(0),
            fail_at: u64::MAX,
            edit_after: Some((1, paths[0].clone())),
        };

        let error = install_snapshots_with(&before, &desired, &operations)
            .expect_err("成功路径清理备份前必须复核目标")
            .to_string();

        assert!(error.contains("安装后文件被并发修改"));
        assert_eq!(
            fs::read_to_string(&paths[0]).unwrap(),
            "manual-after-install"
        );
        assert!(!backup_files(paths[0].parent().unwrap()).is_empty());
    }

    #[test]
    fn candidate_sync_rolls_back_all_managed_files_on_failure() {
        let frontend = TestFrontend::new();
        let generated = frontend.0.join(CANDIDATE_MANAGED_PATHS[1]);
        let source = frontend.0.join("openapi/source.json");
        let result = apply_candidate(&frontend.backend(), &frontend.0, candidate(), |staging| {
            fs::write(staging.join(CANDIDATE_MANAGED_PATHS[1]), "partial")?;
            fs::write(staging.join("openapi/source.json"), "mutated")?;
            Err("模拟派生文件生成失败".into())
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(frontend.0.join("openapi/openapi.json")).unwrap(),
            "original:openapi/openapi.json"
        );
        assert_eq!(
            fs::read_to_string(generated).unwrap(),
            format!("original:{}", CANDIDATE_MANAGED_PATHS[1])
        );
        assert_eq!(fs::read_to_string(source).unwrap(), "formal-source");
        assert_eq!(
            fs::read_to_string(frontend.backend().join("openapi/openapi.json")).unwrap(),
            "original-backend-openapi"
        );
        assert!(!frontend.0.join("openapi/candidate.json").exists());
    }

    #[test]
    fn candidate_sync_refuses_to_overwrite_concurrent_manual_edit() {
        let frontend = TestFrontend::new();
        let generated = frontend.0.join(CANDIDATE_MANAGED_PATHS[1]);
        let result = apply_candidate(&frontend.backend(), &frontend.0, candidate(), |_| {
            fs::write(&generated, "manual-edit")?;
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&generated).unwrap(), "manual-edit");
        assert_eq!(
            fs::read_to_string(frontend.0.join("openapi/openapi.json")).unwrap(),
            "original:openapi/openapi.json"
        );
        assert_eq!(
            fs::read_to_string(frontend.backend().join("openapi/openapi.json")).unwrap(),
            "original-backend-openapi"
        );
        assert!(!frontend.0.join("openapi/candidate.json").exists());
    }

    #[test]
    fn candidate_snapshot_precedes_staging_copy_and_preserves_interleaved_edit() {
        let frontend = TestFrontend::new();
        let generated = frontend.0.join(CANDIDATE_MANAGED_PATHS[1]);
        let result = apply_candidate_with_staging_hook(
            &frontend.backend(),
            &frontend.0,
            candidate(),
            |_| {
                fs::write(&generated, "manual-between-stage-and-install")?;
                Ok(())
            },
            |_| Ok(()),
        );

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&generated).unwrap(),
            "manual-between-stage-and-install"
        );
        assert_eq!(
            fs::read_to_string(frontend.backend().join("openapi/openapi.json")).unwrap(),
            "original-backend-openapi"
        );
        assert!(!frontend.0.join("openapi/candidate.json").exists());
    }

    #[test]
    fn interrupted_contract_artifacts_block_a_new_sync() {
        let frontend = TestFrontend::new();
        let target = frontend.0.join("openapi/openapi.json");
        let backup = frontend
            .0
            .join("openapi/.openapi.json.xtask-backup-999-123456-0");
        fs::write(&backup, "recoverable-openapi").unwrap();

        let error = apply_candidate(&frontend.backend(), &frontend.0, candidate(), |_| Ok(()))
            .expect_err("事务遗留存在时必须安全失败")
            .to_string();

        assert!(error.contains("上次契约事务未完整结束"));
        assert!(
            error.contains(backup.file_name().unwrap().to_str().unwrap()),
            "错误应包含可恢复文件路径：{error}"
        );
        assert_eq!(
            fs::read_to_string(target).unwrap(),
            "original:openapi/openapi.json"
        );
        assert_eq!(fs::read_to_string(&backup).unwrap(), "recoverable-openapi");
    }

    #[test]
    fn atomic_write_restores_original_when_install_fails() {
        let frontend = TestFrontend::new();
        let target = frontend.0.join("atomic.txt");
        fs::write(&target, "old").unwrap();
        let operations = AtomicFaults {
            fail_backup_cleanup: false,
            fail_install: true,
            fail_restore: false,
            target: target.clone(),
        };

        assert!(write_atomically_with(&target, b"new", &operations).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "old");
        assert!(backup_files(&frontend.0).is_empty());
    }

    #[test]
    fn atomic_write_reports_failed_restore_and_preserves_backup() {
        let frontend = TestFrontend::new();
        let target = frontend.0.join("atomic.txt");
        fs::write(&target, "old").unwrap();
        let operations = AtomicFaults {
            fail_backup_cleanup: false,
            fail_install: true,
            fail_restore: true,
            target: target.clone(),
        };

        let error = write_atomically_with(&target, b"new", &operations)
            .expect_err("恢复失败必须显式报告")
            .to_string();
        assert!(error.contains("原文件备份保留"));
        assert!(!target.exists());
        let backups = backup_files(&frontend.0);
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "old");
    }

    #[test]
    fn atomic_write_keeps_success_when_only_backup_cleanup_fails() {
        let frontend = TestFrontend::new();
        let target = frontend.0.join("atomic.txt");
        fs::write(&target, "old").unwrap();
        let operations = AtomicFaults {
            fail_backup_cleanup: true,
            fail_install: false,
            fail_restore: false,
            target: target.clone(),
        };

        write_atomically_with(&target, b"new", &operations).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        let backups = backup_files(&frontend.0);
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read_to_string(&backups[0]).unwrap(), "old");
    }

    #[test]
    fn formal_sync_requires_pinned_source_and_matching_content_hash() {
        let frontend = TestFrontend::new();
        let openapi = candidate();
        fs::write(frontend.0.join("openapi/openapi.json"), openapi).unwrap();
        let hash = sha256_hex(openapi);
        let metadata = serde_json::json!({
            "schema_version": 1,
            "backend_repository": "Edgar-ycy/ryframe",
            "backend_commit": "0123456789abcdef0123456789abcdef01234567",
            "openapi_path": "openapi/openapi.json",
            "openapi_version": "3.1.0",
            "sha256": hash,
        });
        fs::write(
            frontend.0.join("openapi/source.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        let artifacts = CANDIDATE_MANAGED_PATHS[1..]
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>();

        validate_formal_sync(
            &frontend.0,
            "Edgar-ycy/ryframe",
            "0123456789abcdef0123456789abcdef01234567",
            &artifacts,
        )
        .unwrap();

        let mut invalid = metadata;
        invalid["sha256"] = serde_json::Value::String("0".repeat(64));
        fs::write(
            frontend.0.join("openapi/source.json"),
            serde_json::to_vec(&invalid).unwrap(),
        )
        .unwrap();
        assert!(
            validate_formal_sync(
                &frontend.0,
                "Edgar-ycy/ryframe",
                "0123456789abcdef0123456789abcdef01234567",
                &artifacts,
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_non_ryframe_candidate() {
        assert!(validate_candidate_contract(br#"{"openapi":"2.0"}"#).is_err());
    }

    #[test]
    fn reads_frontend_generated_artifact_manifest_and_rejects_traversal() {
        let frontend = TestFrontend::new();
        assert_eq!(
            generated_artifact_paths(&frontend.0).unwrap(),
            CANDIDATE_MANAGED_PATHS[1..]
        );
        fs::write(
            frontend.0.join("scripts/api-artifacts.mjs"),
            "export const generatedArtifactPaths = Object.freeze([\n  '../outside.ts',\n])\n",
        )
        .unwrap();
        assert!(generated_artifact_paths(&frontend.0).is_err());
    }
}

mod doctor_tests {
    use super::doctor::{ToolVersion, node_engine_satisfies, parse_python_version};

    fn version(value: &str) -> ToolVersion {
        ToolVersion::parse(value, "测试版本").unwrap()
    }

    #[test]
    fn parses_tool_versions() {
        assert_eq!(version("v22.22.2"), version("22.22.2"));
        assert!(ToolVersion::parse("22.22", "测试版本").is_err());
        assert_eq!(
            parse_python_version("Python 3.12.1\n").unwrap(),
            version("3.12.1")
        );
    }

    #[test]
    fn validates_node_version_ranges() {
        let range = "^22.22.2 || >=24.15.0";
        assert!(node_engine_satisfies(version("22.22.2"), range).unwrap());
        assert!(!node_engine_satisfies(version("23.0.0"), range).unwrap());
        assert!(node_engine_satisfies(version("24.15.0"), range).unwrap());
    }
}

mod migration_tests {
    use std::{
        fs, io,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{
        cli::MigrationScope,
        migration::{FileOperations, PlannedWrite, commit_writes_with, create_migration},
    };

    static NEXT_DIR: AtomicU64 = AtomicU64::new(1);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ryframe-xtask-migration-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, relative: &str, content: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FaultOperations {
        fail_install: bool,
        fail_restore: bool,
        fail_backup_cleanup: bool,
    }

    impl FaultOperations {
        fn has_role(path: &Path, role: &str) -> bool {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(&format!(".xtask-{role}-")))
        }
    }

    impl FileOperations for FaultOperations {
        fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
            if self.fail_restore && Self::has_role(source, "backup") {
                return Err(io::Error::other("注入 backup→target 恢复失败"));
            }
            fs::rename(source, target)
        }

        fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
            if self.fail_install && Self::has_role(source, "new") {
                return Err(io::Error::other("注入 staged→target 失败"));
            }
            fs::hard_link(source, target)
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            if self.fail_backup_cleanup && Self::has_role(path, "backup") {
                return Err(io::Error::other("注入 backup 清理失败"));
            }
            fs::remove_file(path)
        }
    }

    struct NthInstallFaults {
        hard_link_calls: AtomicU64,
        fail_at: u64,
        edit_after: Option<(u64, PathBuf)>,
    }

    impl FileOperations for NthInstallFaults {
        fn rename(&self, source: &Path, target: &Path) -> io::Result<()> {
            fs::rename(source, target)
        }

        fn hard_link(&self, source: &Path, target: &Path) -> io::Result<()> {
            let call = self.hard_link_calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == self.fail_at {
                return Err(io::Error::other("注入第 N 个迁移文件安装失败"));
            }
            fs::hard_link(source, target)?;
            if self
                .edit_after
                .as_ref()
                .is_some_and(|(at, path)| *at == call && path == target)
            {
                fs::remove_file(target)?;
                fs::write(target, b"manual-after-install")?;
            }
            Ok(())
        }

        fn remove_file(&self, path: &Path) -> io::Result<()> {
            fs::remove_file(path)
        }
    }

    #[test]
    fn creates_control_migration_and_registers_it_once() {
        let root = TestRoot::new();
        root.write(
            "crates/ryframe-db/src/migration/mod.rs",
            "mod m20260820_000000_control_baseline;\nmod schema;\n\nimpl MigratorTrait for Migrator {\n    fn migrations() -> Vec<Box<dyn MigrationTrait>> {\n        vec![Box::new(m20260820_000000_control_baseline::Migration)]\n    }\n}\n",
        );
        create_migration(
            &root.0,
            MigrationScope::Control,
            "add_device",
            "20260823_010203",
        )
        .unwrap();
        let migration = fs::read_to_string(
            root.0
                .join("crates/ryframe-db/src/migration/m20260823_010203_add_device.rs"),
        )
        .unwrap();
        assert!(migration.contains("尚未实现"));
        assert!(migration.contains("追加迁移 m20260823_010203_add_device 不支持 down"));
        let registry =
            fs::read_to_string(root.0.join("crates/ryframe-db/src/migration/mod.rs")).unwrap();
        assert!(registry.contains("mod m20260823_010203_add_device;"));
        assert!(registry.contains("Box::new(m20260823_010203_add_device::Migration)"));
        assert!(
            create_migration(
                &root.0,
                MigrationScope::Control,
                "add_device",
                "20260823_010203",
            )
            .is_err()
        );
    }

    #[test]
    fn tenant_migration_updates_module_and_runtime_registries() {
        let root = TestRoot::new();
        root.write(
            "crates/ryframe-tenant-db/src/migration/mod.rs",
            "mod m20260820_000000_tenant_baseline;\nmod runtime;\n",
        );
        root.write(
            "crates/ryframe-tenant-db/src/migration/runtime.rs",
            "impl MigratorTrait for Migrator {\n    fn migrations() -> Vec<Box<dyn MigrationTrait>> {\n        vec![Box::new(super::m20260820_000000_tenant_baseline::Migration)]\n    }\n}\n",
        );
        create_migration(
            &root.0,
            MigrationScope::TenantData,
            "add_device",
            "20260823_010204",
        )
        .unwrap();
        let modules =
            fs::read_to_string(root.0.join("crates/ryframe-tenant-db/src/migration/mod.rs"))
                .unwrap();
        let runtime = fs::read_to_string(
            root.0
                .join("crates/ryframe-tenant-db/src/migration/runtime.rs"),
        )
        .unwrap();
        assert!(modules.contains("mod m20260823_010204_add_device;"));
        assert!(runtime.contains("Box::new(super::m20260823_010204_add_device::Migration)"));
    }

    #[test]
    fn invalid_migration_path_leaves_registries_unchanged() {
        let root = TestRoot::new();
        let registry = "mod m20260820_000000_control_baseline;\n\nimpl MigratorTrait for Migrator {\n    fn migrations() -> Vec<Box<dyn MigrationTrait>> {\n        vec![Box::new(m20260820_000000_control_baseline::Migration)]\n    }\n}\n";
        root.write("crates/ryframe-db/src/migration/mod.rs", registry);

        assert!(
            create_migration(
                &root.0,
                MigrationScope::Control,
                "../escape",
                "20260823_010203",
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.0.join("crates/ryframe-db/src/migration/mod.rs")).unwrap(),
            registry
        );
        assert!(!root.0.join("crates/ryframe-db/src/escape.rs").exists());
    }

    #[test]
    fn migration_timestamp_must_advance_without_partial_writes() {
        let root = TestRoot::new();
        let registry = "mod m20260820_000000_control_baseline;\n\nimpl MigratorTrait for Migrator {\n    fn migrations() -> Vec<Box<dyn MigrationTrait>> {\n        vec![Box::new(m20260820_000000_control_baseline::Migration)]\n    }\n}\n";
        root.write("crates/ryframe-db/src/migration/mod.rs", registry);
        root.write(
            "crates/ryframe-db/src/migration/m20260823_020000_existing.rs",
            "existing",
        );

        assert!(
            create_migration(
                &root.0,
                MigrationScope::Control,
                "add_device",
                "20260823_010203",
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.0.join("crates/ryframe-db/src/migration/mod.rs")).unwrap(),
            registry
        );
        assert!(
            !root
                .0
                .join("crates/ryframe-db/src/migration/m20260823_010203_add_device.rs")
                .exists()
        );
    }

    #[test]
    fn failed_install_restores_the_current_backup_through_shared_rollback() {
        let root = TestRoot::new();
        let target = root.0.join("registry.rs");
        fs::write(&target, b"before\n").unwrap();
        let writes = [PlannedWrite {
            path: target.clone(),
            expected: Some(b"before\n".to_vec()),
            content: b"after\n".to_vec(),
        }];
        let operations = FaultOperations {
            fail_install: true,
            fail_restore: false,
            fail_backup_cleanup: false,
        };

        let error = commit_writes_with(&writes, &operations)
            .expect_err("安装失败必须进入统一回滚")
            .to_string();

        assert!(error.contains("注入 staged→target 失败"));
        assert_eq!(fs::read(&target).unwrap(), b"before\n");
        assert!(
            fs::read_dir(&root.0)
                .unwrap()
                .all(|entry| !FaultOperations::has_role(&entry.unwrap().path(), "backup"))
        );
    }

    #[test]
    fn failed_restore_is_reported_and_keeps_the_backup_recoverable() {
        let root = TestRoot::new();
        let target = root.0.join("registry.rs");
        fs::write(&target, b"before\n").unwrap();
        let writes = [PlannedWrite {
            path: target.clone(),
            expected: Some(b"before\n".to_vec()),
            content: b"after\n".to_vec(),
        }];
        let operations = FaultOperations {
            fail_install: true,
            fail_restore: true,
            fail_backup_cleanup: false,
        };

        let error = commit_writes_with(&writes, &operations)
            .expect_err("恢复失败必须与安装首错合并")
            .to_string();

        assert!(error.contains("注入 staged→target 失败"));
        assert!(error.contains("回滚未能安全完成"));
        assert!(error.contains("注入 backup→target 恢复失败"));
        assert!(error.contains("备份保留在"));
        assert!(!target.exists());
        assert!(
            fs::read_dir(&root.0)
                .unwrap()
                .any(|entry| FaultOperations::has_role(&entry.unwrap().path(), "backup"))
        );
    }

    #[test]
    fn backup_cleanup_failure_does_not_claim_the_new_target_was_rolled_back() {
        let root = TestRoot::new();
        let target = root.0.join("registry.rs");
        fs::write(&target, b"before\n").unwrap();
        let writes = [PlannedWrite {
            path: target.clone(),
            expected: Some(b"before\n".to_vec()),
            content: b"after\n".to_vec(),
        }];
        let operations = FaultOperations {
            fail_install: false,
            fail_restore: false,
            fail_backup_cleanup: true,
        };

        let error = commit_writes_with(&writes, &operations)
            .expect_err("备份清理失败必须可观测")
            .to_string();

        assert!(error.contains("迁移文件已写入，但清理备份失败"));
        assert!(error.contains("目标文件保持新内容"));
        assert_eq!(fs::read(&target).unwrap(), b"after\n");
    }

    #[test]
    fn later_migration_install_failure_restores_every_previous_file() {
        let root = TestRoot::new();
        let paths = [
            root.0.join("migration.rs"),
            root.0.join("mod.rs"),
            root.0.join("runtime.rs"),
        ];
        for (index, path) in paths.iter().enumerate() {
            fs::write(path, format!("before-{index}")).unwrap();
        }
        let writes = paths
            .iter()
            .enumerate()
            .map(|(index, path)| PlannedWrite {
                path: path.clone(),
                expected: Some(format!("before-{index}").into_bytes()),
                content: format!("after-{index}").into_bytes(),
            })
            .collect::<Vec<_>>();
        let operations = NthInstallFaults {
            hard_link_calls: AtomicU64::new(0),
            fail_at: 2,
            edit_after: None,
        };

        let error = commit_writes_with(&writes, &operations)
            .expect_err("第二个文件安装失败必须回滚")
            .to_string();

        assert!(error.contains("第 N 个迁移文件安装失败"));
        for (index, path) in paths.iter().enumerate() {
            assert_eq!(fs::read_to_string(path).unwrap(), format!("before-{index}"));
        }
        assert!(
            fs::read_dir(&root.0)
                .unwrap()
                .filter_map(std::result::Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().contains(".xtask-"))
        );
    }

    #[test]
    fn migration_rollback_preserves_manual_edit_and_blocks_retry() {
        let root = TestRoot::new();
        let paths = [
            root.0.join("migration.rs"),
            root.0.join("mod.rs"),
            root.0.join("runtime.rs"),
        ];
        for (index, path) in paths.iter().enumerate() {
            fs::write(path, format!("before-{index}")).unwrap();
        }
        let writes = paths
            .iter()
            .enumerate()
            .map(|(index, path)| PlannedWrite {
                path: path.clone(),
                expected: Some(format!("before-{index}").into_bytes()),
                content: format!("after-{index}").into_bytes(),
            })
            .collect::<Vec<_>>();
        let operations = NthInstallFaults {
            hard_link_calls: AtomicU64::new(0),
            fail_at: 2,
            edit_after: Some((1, paths[0].clone())),
        };

        let error = commit_writes_with(&writes, &operations)
            .expect_err("并发编辑后续失败必须保留恢复状态")
            .to_string();

        assert!(error.contains("再次修改"));
        assert_eq!(
            fs::read_to_string(&paths[0]).unwrap(),
            "manual-after-install"
        );
        assert!(
            fs::read_dir(&root.0)
                .unwrap()
                .filter_map(std::result::Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".xtask-migration-transaction-"))
        );
        let retry = FaultOperations {
            fail_install: false,
            fail_restore: false,
            fail_backup_cleanup: false,
        };
        assert!(
            commit_writes_with(&writes, &retry)
                .expect_err("恢复状态存在时必须拒绝重试")
                .to_string()
                .contains("上次迁移文件事务未完整结束")
        );
    }

    #[test]
    fn migration_success_path_rechecks_targets_before_backup_cleanup() {
        let root = TestRoot::new();
        let paths = [root.0.join("migration.rs"), root.0.join("mod.rs")];
        for (index, path) in paths.iter().enumerate() {
            fs::write(path, format!("before-{index}")).unwrap();
        }
        let writes = paths
            .iter()
            .enumerate()
            .map(|(index, path)| PlannedWrite {
                path: path.clone(),
                expected: Some(format!("before-{index}").into_bytes()),
                content: format!("after-{index}").into_bytes(),
            })
            .collect::<Vec<_>>();
        let operations = NthInstallFaults {
            hard_link_calls: AtomicU64::new(0),
            fail_at: u64::MAX,
            edit_after: Some((1, paths[0].clone())),
        };

        let error = commit_writes_with(&writes, &operations)
            .expect_err("成功路径清理备份前必须复核目标")
            .to_string();

        assert!(error.contains("安装后文件被并发修改"));
        assert_eq!(
            fs::read_to_string(&paths[0]).unwrap(),
            "manual-after-install"
        );
        assert!(
            fs::read_dir(&root.0)
                .unwrap()
                .filter_map(std::result::Result::ok)
                .any(|entry| FaultOperations::has_role(&entry.path(), "backup"))
        );
    }

    #[test]
    fn interrupted_transaction_artifacts_block_a_new_migration_write() {
        let root = TestRoot::new();
        let target = root.0.join("registry.rs");
        fs::write(&target, b"before\n").unwrap();
        let backup = root.0.join(".registry.rs.xtask-backup-999-123456-0");
        fs::write(&backup, b"recoverable-before\n").unwrap();
        let writes = [PlannedWrite {
            path: target.clone(),
            expected: Some(b"before\n".to_vec()),
            content: b"after\n".to_vec(),
        }];
        let operations = FaultOperations {
            fail_install: false,
            fail_restore: false,
            fail_backup_cleanup: false,
        };

        let error = commit_writes_with(&writes, &operations)
            .expect_err("事务遗留存在时必须安全失败")
            .to_string();

        assert!(error.contains("上次迁移文件事务未完整结束"));
        assert!(error.contains(&backup.display().to_string()));
        assert_eq!(fs::read(&target).unwrap(), b"before\n");
        assert_eq!(fs::read(&backup).unwrap(), b"recoverable-before\n");
    }
}

mod watch_tests {
    use std::{fs, time::Duration};

    use super::watch::{SourceWatcher, WatchEvent, is_backend_watch_path};

    #[test]
    fn watches_backend_inputs_but_ignores_build_outputs_and_docs() {
        for path in [
            "Cargo.toml",
            "crates/ryframe/src/main.rs",
            "crates/ryframe/Cargo.toml",
            "config/app.dev.toml",
            "catalog/resources/post.toml",
            "locales/zh-CN.toml",
        ] {
            assert!(is_backend_watch_path(path), "应监听 {path}");
        }
        for path in [
            "target/debug/ryframe.exe",
            ".git/index",
            ".local-tests/result.json",
            "docs/development.md",
            "README.md",
        ] {
            assert!(!is_backend_watch_path(path), "不应监听 {path}");
        }
    }

    #[test]
    fn source_watcher_reports_backend_file_change_and_stops_cleanly() {
        let root = std::env::temp_dir().join(format!("ryframe-xtask-watch-{}", std::process::id()));
        let source = root.join("crates/demo/src/lib.rs");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "pub fn before() {}\n").unwrap();
        let watcher = SourceWatcher::new(&root).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        fs::write(&source, "pub fn after() {}\n").unwrap();
        let event = watcher.recv_timeout(Duration::from_secs(3)).unwrap();
        let Some(WatchEvent::BackendChanged(path)) = event else {
            panic!("应收到后端源码变更");
        };
        let event = watcher
            .drain_changes(path, Duration::from_millis(100))
            .unwrap();

        assert!(matches!(
            event,
            WatchEvent::BackendChanged(path) if path == "crates/demo/src/lib.rs"
        ));
        drop(watcher);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
mod process_tests {
    use std::process::{Command, Stdio};

    use super::process::ChildGroup;

    #[test]
    fn windows_job_object_accepts_and_waits_for_child() {
        let group = ChildGroup::new().unwrap();
        let mut command = Command::new("cmd");
        command
            .args(["/C", "exit", "0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = group.spawn(&mut command).unwrap().wait().unwrap();
        assert!(status.success());
    }
}

#[test]
fn default_frontend_is_sibling_of_backend() {
    let root = workspace::root_dir();
    assert_eq!(
        workspace::default_frontend_dir(),
        root.parent().unwrap().join("ryframe-vue3")
    );
}

#[test]
fn daily_cargo_aliases_lock_dependencies_and_isolate_the_runner() {
    let config = std::fs::read_to_string(workspace::root_dir().join(".cargo/config.toml")).unwrap();
    for alias in ["xtask", "dev", "verify", "api-sync", "migrate"] {
        let prefix = format!("{alias} = \"run --locked --target-dir target/xtask-run ");
        assert!(config.lines().any(|line| line.starts_with(&prefix)));
    }
    assert!(config.lines().any(|line| {
        line.starts_with("resource = \"run --locked --target-dir target/xtask-resource ")
    }));
}

#[cfg(feature = "resource")]
#[test]
fn loads_and_explains_catalog_resources() {
    let resources = resource::load_catalog(&workspace::root_dir()).unwrap();
    let catalog = ryframe_generator::render_resources(&resources).unwrap();
    assert!(catalog.explanation("post").is_some());
}
