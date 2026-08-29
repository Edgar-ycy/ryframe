use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Barrier,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::dev::{
    ArtifactAction, BuildContext, BuildPlan, BuildResult, CandidateProbeDisposition, ChangeKind,
    ChangeOutcome, CycleControl, DEV_API_FEATURES, DevSession, MigrationValidation, ProbeResult,
    ReadyKind, RuntimeInputPaths, RuntimeSecrets, SaveCase, StepResult,
    TOOL_SELF_CHANGED_EXIT_CODE, api_command, available_ports, build_candidate,
    candidate_probe_disposition, classify_change, combine_failures, failure_exit_code, ready_kind,
    run_migration_validation, start_worker_after_api_ready, switch_services_with_rollback,
    tool_self_changed_error, wait_services_ready_until, wait_services_ready_until_controlled,
    worker_command,
};
use super::{
    process::ChildGroup,
    watch::{ChangeBatch, SourceRevision, SourceRevisionTracker, SourceWatcher},
};

#[test]
fn development_changes_are_classified_by_runtime_impact() {
    for (path, expected) in [
        ("config/app.dev.toml", ChangeKind::RuntimeConfig),
        ("config/app.prod.toml", ChangeKind::IgnoredDevConfig),
        ("locales/zh-CN.toml", ChangeKind::LocaleCatalog),
        ("crates/ryframe-api/src/lib.rs", ChangeKind::ApiOnly),
        ("crates/ryframe/src/app.rs", ChangeKind::ApiOnly),
        ("crates/ryframe/src/app/router.rs", ChangeKind::ApiOnly),
        (
            "crates/ryframe/src/bin/ryframe_worker.rs",
            ChangeKind::WorkerOnly,
        ),
        (
            "crates/ryframe/src/bin/ryframe_worker/process.rs",
            ChangeKind::WorkerOnly,
        ),
        (
            "crates/ryframe-application/src/lib.rs",
            ChangeKind::SharedRuntime,
        ),
        ("crates/ryframe-macro/src/lib.rs", ChangeKind::SharedRuntime),
        ("crates/ryframe-db/src/lib.rs", ChangeKind::SharedRuntime),
        (
            "crates/ryframe-db/src/migration/mod.rs",
            ChangeKind::ControlPersistence,
        ),
        (
            "crates/ryframe-tenant-db/src/lib.rs",
            ChangeKind::SharedRuntime,
        ),
        (
            "crates/ryframe-tenant-db/src/migration/mod.rs",
            ChangeKind::TenantPersistence,
        ),
        ("catalog/resources/post.toml", ChangeKind::ResourceManifest),
        ("xtask/src/dev.rs", ChangeKind::ToolSelf),
        ("xtask/Cargo.toml", ChangeKind::ToolSelf),
        ("xtask/build.rs", ChangeKind::ToolSelf),
        ("vendor/sqlx-mysql-only/src/lib.rs", ChangeKind::BuildGraph),
        (
            "crates/future-backend/src/lib.rs",
            ChangeKind::UnknownBackend,
        ),
        ("Cargo.lock", ChangeKind::BuildGraph),
    ] {
        assert_eq!(classify_change(path), expected, "错误分类 {path}");
    }
}

#[test]
fn macro_changes_rebuild_both_runtime_targets() {
    let plan = BuildPlan::from_changes(&ChangeBatch {
        revision: SourceRevision::from_value(1),
        paths: BTreeSet::from(["crates/ryframe-macro/src/lib.rs".to_owned()]),
    });

    assert_eq!(plan.api, ArtifactAction::Rebuild);
    assert_eq!(plan.worker, ArtifactAction::Rebuild);
    assert!(plan.restart_pair);
    assert!(!plan.resource_check);
}

#[test]
fn tool_self_change_has_a_stable_rerun_exit_code() {
    let error = tool_self_changed_error();

    assert_eq!(failure_exit_code(error.as_ref()), Some(75));
    assert_eq!(TOOL_SELF_CHANGED_EXIT_CODE, 75);
    assert_eq!(
        error.to_string(),
        "xtask 自身已变化，请重新运行 `cargo dev`"
    );
    let ordinary = std::io::Error::other("ordinary failure");
    assert_eq!(failure_exit_code(&ordinary), None);
}

#[test]
fn persistence_changes_have_distinct_build_and_verification_plans() {
    struct Case {
        label: &'static str,
        paths: &'static [&'static str],
        runtime: ArtifactAction,
        restart_pair: bool,
        migration: MigrationValidation,
    }
    let cases = [
        Case {
            label: "control",
            paths: &["crates/ryframe-db/src/migration/m20260827.rs"],
            runtime: ArtifactAction::Rebuild,
            restart_pair: true,
            migration: MigrationValidation::StandaloneControl,
        },
        Case {
            label: "tenant",
            paths: &["crates/ryframe-tenant-db/src/migration/m20260827.rs"],
            runtime: ArtifactAction::NotNeeded,
            restart_pair: false,
            migration: MigrationValidation::StandaloneTenant,
        },
        Case {
            label: "control+tenant",
            paths: &[
                "crates/ryframe-db/src/migration/m20260827.rs",
                "crates/ryframe-tenant-db/src/migration/m20260827.rs",
            ],
            runtime: ArtifactAction::Rebuild,
            restart_pair: true,
            migration: MigrationValidation::StandaloneControlAndTenant,
        },
    ];

    for (revision, case) in cases.into_iter().enumerate() {
        let batch = ChangeBatch {
            revision: SourceRevision::from_value(revision as u64 + 1),
            paths: case.paths.iter().map(|path| (*path).to_owned()).collect(),
        };
        let plan = BuildPlan::from_changes(&batch);
        assert_eq!(plan.api, case.runtime, "{} API 计划错误", case.label);
        assert_eq!(plan.worker, case.runtime, "{} Worker 计划错误", case.label);
        assert_eq!(
            plan.restart_pair, case.restart_pair,
            "{} 重启计划错误",
            case.label
        );
        assert!(plan.migrate, "{} 必须构建迁移验证目标", case.label);
        assert_eq!(
            plan.migration, case.migration,
            "{} 验证计划错误",
            case.label
        );
    }
}

#[test]
fn migration_validation_is_superseded_during_the_running_process() {
    let root = std::env::temp_dir().join(format!(
        "ryframe-xtask-migration-supersede-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("locales")).unwrap();
    let config = root.join("config/app.dev.toml");
    fs::write(&config, "[app]\nport = 8080\n").unwrap();
    let migrate = write_slow_migration_stub(&root);
    let watcher = SourceWatcher::new(&root).unwrap();
    thread::sleep(Duration::from_millis(100));
    let plan = BuildPlan::from_changes(&ChangeBatch {
        revision: watcher.current_revision(),
        paths: ["crates/ryframe/src/bin/ryframe_migrate.rs".to_owned()]
            .into_iter()
            .collect(),
    });
    let changed_config = config;
    let changed = thread::spawn(move || {
        thread::sleep(Duration::from_millis(250));
        fs::write(changed_config, "[app]\nport = 8081\n").unwrap();
    });
    let group = ChildGroup::new().unwrap();
    let shutdown = AtomicBool::new(false);
    let mut lkg_check = None;
    let started = Instant::now();

    let result = run_migration_validation(
        &group,
        &root,
        &migrate,
        &root.join("config"),
        &root.join("locales"),
        &RuntimeSecrets::default(),
        &shutdown,
        &watcher,
        &plan,
        &mut lkg_check,
    )
    .unwrap();

    changed.join().unwrap();
    assert_eq!(result, StepResult::Superseded);
    assert!(started.elapsed() < Duration::from_secs(3));
    drop(watcher);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn compile_build_is_superseded_after_fake_cargo_starts() {
    let root = std::env::temp_dir().join(format!(
        "ryframe-xtask-compile-supersede-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("config")).unwrap();
    fs::create_dir_all(root.join("locales")).unwrap();
    let marker = root.join(".fake-cargo-started");
    let fake_cargo = write_blocking_cargo_stub(&root);
    let watcher = SourceWatcher::new(&root).unwrap();
    let plan = BuildPlan::from_changes(&ChangeBatch {
        revision: watcher.current_revision(),
        paths: ["crates/ryframe-api/src/lib.rs".to_owned()]
            .into_iter()
            .collect(),
    });
    let session =
        DevSession::prepare_isolated(&root.join("state"), watcher.current_revision()).unwrap();
    let group = ChildGroup::new().unwrap();
    let shutdown = AtomicBool::new(false);
    let cargo_invocations = AtomicUsize::new(0);
    let context = BuildContext::new(&group, &root, &session, &shutdown, &watcher, &fake_cargo)
        .with_cargo_counter(&cargo_invocations);
    let started = Instant::now();
    let mut lkg_check = || {
        if started.elapsed() > Duration::from_secs(3) {
            return Err("fake Cargo 未进入可取消的编译阶段".into());
        }
        Ok(())
    };

    let result = build_candidate(&context, &plan, None, Some(&mut lkg_check)).unwrap();

    assert!(matches!(result, BuildResult::Superseded));
    assert_eq!(cargo_invocations.load(Ordering::Relaxed), 1);
    assert!(marker.is_file(), "测试必须先确认 fake Cargo 已启动");
    assert!(watcher.current_revision() > plan.source_revision);
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(watcher);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn final_revision_fence_rejects_stale_and_queues_later_changes() {
    let stale = SourceRevisionTracker::default();
    let expected = stale.current_revision();
    record_test_change(&stale);
    let mut stale_promotions = 0;
    if stale.final_revision_fence(expected) {
        stale_promotions += 1;
    }
    assert_eq!(stale_promotions, 0, "过期代次不得进入 promotion");

    let queued = SourceRevisionTracker::default();
    let expected = queued.current_revision();
    assert!(queued.final_revision_fence(expected));
    record_test_change(&queued);
    assert!(
        queued.current_revision() > expected,
        "fence 后事件应留给下一轮"
    );
}

#[test]
fn final_revision_fence_has_no_stale_promotion_across_128_races() {
    for _ in 0..128 {
        let tracker = Arc::new(SourceRevisionTracker::default());
        let expected = tracker.current_revision();
        let barrier = Arc::new(Barrier::new(2));
        let changed_tracker = Arc::clone(&tracker);
        let changed_barrier = Arc::clone(&barrier);
        let changed = thread::spawn(move || {
            changed_barrier.wait();
            record_test_change(&changed_tracker)
        });
        let promotions = AtomicUsize::new(0);

        barrier.wait();
        let allowed = tracker.final_revision_fence(expected);
        if allowed {
            promotions.fetch_add(1, Ordering::Relaxed);
        }
        let changed_revision = changed.join().expect("变更线程不应失败");

        assert!(changed_revision > expected);
        if !allowed {
            assert_eq!(
                promotions.load(Ordering::Relaxed),
                0,
                "过期代次 promotion 必须为零"
            );
        } else {
            assert_eq!(
                promotions.load(Ordering::Relaxed),
                1,
                "fence 后事件属于下一轮队列"
            );
        }
    }
}

fn record_test_change(tracker: &SourceRevisionTracker) -> SourceRevision {
    let root = Path::new("D:/workspace");
    tracker
        .record_paths(root, &[root.join("Cargo.toml")])
        .expect("测试变更应被 watcher 接受")
        .0
}

#[test]
fn save_cases_drive_the_real_change_plan_and_cargo_count() {
    assert_eq!(DEV_API_FEATURES, "bin-api,runtime-swagger-ui");
    for (case, cargo_invocations, expected_ready) in [
        (SaveCase::ConfigOnly, 0, ReadyKind::Promoted),
        (SaveCase::ApiOnly, 1, ReadyKind::Promoted),
        (SaveCase::WorkerOnly, 1, ReadyKind::Promoted),
        (SaveCase::SharedRuntime, 2, ReadyKind::Promoted),
        (SaveCase::Locales, 2, ReadyKind::Promoted),
        (SaveCase::MigrationOnly, 1, ReadyKind::VerifiedNoRestart),
        (SaveCase::ResourceManifest, 1, ReadyKind::VerifiedNoRestart),
    ] {
        let source = case.source();
        let batch = ChangeBatch {
            revision: SourceRevision::from_value(1),
            paths: BTreeSet::from([source.relative.to_owned()]),
        };
        let plan = BuildPlan::from_changes(&batch);
        let outcome = if plan.restart_pair {
            ChangeOutcome::Promoted
        } else {
            ChangeOutcome::VerifiedNoRestart
        };
        assert_eq!(plan.cargo_invocations(), cargo_invocations, "{case:?}");
        assert_eq!(
            ready_kind(case, outcome).unwrap(),
            expected_ready,
            "{case:?}"
        );
    }
    assert_eq!(
        SaveCase::parse("cancellation"),
        Some(SaveCase::Cancellation)
    );
    assert_eq!(SaveCase::Cancellation.expected_cargo_invocations(), 1);
    assert_eq!(
        ready_kind(SaveCase::Cancellation, ChangeOutcome::Superseded).unwrap(),
        ReadyKind::Superseded
    );
    assert!(SaveCase::parse("unknown").is_none());
    assert!(ready_kind(SaveCase::ConfigOnly, ChangeOutcome::Failed).is_err());
    assert!(ready_kind(SaveCase::MigrationOnly, ChangeOutcome::Promoted).is_err());
}

#[test]
fn development_plan_unions_every_path_in_the_batch() {
    let batch = ChangeBatch {
        revision: SourceRevision::from_value(7),
        paths: [
            "crates/ryframe-api/src/lib.rs".to_owned(),
            "crates/ryframe/src/bin/ryframe_worker.rs".to_owned(),
        ]
        .into_iter()
        .collect(),
    };
    let plan = BuildPlan::from_changes(&batch);

    assert_eq!(plan.source_revision.value(), 7);
    assert_eq!(plan.api, ArtifactAction::Rebuild);
    assert_eq!(plan.worker, ArtifactAction::Rebuild);
    assert!(plan.restart_pair);
    assert!(!plan.migrate);
}

#[test]
fn unknown_backend_rebuilds_runtime_pair_without_migration_validation() {
    let plan = BuildPlan::from_changes(&ChangeBatch {
        revision: SourceRevision::from_value(8),
        paths: ["crates/future-backend/src/lib.rs".to_owned()]
            .into_iter()
            .collect(),
    });

    assert_eq!(
        plan.reasons,
        [ChangeKind::UnknownBackend].into_iter().collect()
    );
    assert_eq!(plan.api, ArtifactAction::Rebuild);
    assert_eq!(plan.worker, ArtifactAction::Rebuild);
    assert!(plan.restart_pair);
    assert!(!plan.migrate);
    assert_eq!(plan.migration, MigrationValidation::None);
    assert_eq!(plan.cargo_invocations(), 2);
}

#[test]
fn config_only_plan_reuses_binaries_without_cargo() {
    let batch = ChangeBatch {
        revision: SourceRevision::from_value(3),
        paths: ["config/app.dev.toml".to_owned()].into_iter().collect(),
    };
    let plan = BuildPlan::from_changes(&batch);

    assert_eq!(plan.api, ArtifactAction::ReuseLkg);
    assert_eq!(plan.worker, ArtifactAction::ReuseLkg);
    assert!(plan.restart_pair);
    assert!(!plan.migrate);
    assert!(!plan.resource_check);
}

#[test]
fn resource_only_plan_checks_without_restarting_services() {
    let batch = ChangeBatch {
        revision: SourceRevision::from_value(4),
        paths: ["catalog/resources/post.toml".to_owned()]
            .into_iter()
            .collect(),
    };
    let plan = BuildPlan::from_changes(&batch);

    assert_eq!(plan.api, ArtifactAction::NotNeeded);
    assert_eq!(plan.worker, ArtifactAction::NotNeeded);
    assert!(!plan.restart_pair);
    assert!(plan.resource_check);
}

#[test]
fn worker_probe_uses_isolated_non_consuming_mode() {
    let command = worker_command(
        Path::new("D:/workspace"),
        Path::new("D:/workspace/worker.exe"),
        RuntimeInputPaths::new(
            Path::new("D:/workspace/config"),
            Path::new("D:/workspace/locales"),
            &RuntimeSecrets::default(),
        ),
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
    assert_eq!(
        environment.get("APP_DATABASE_MIGRATION_MODE"),
        Some(&Some("verify".into()))
    );
    assert_eq!(
        environment.get("APP_CONFIG_DIR"),
        Some(&Some("D:/workspace/config".into()))
    );
    assert_eq!(
        environment.get("APP_LOCALES_DIR"),
        Some(&Some("D:/workspace/locales".into()))
    );
}

#[test]
fn api_probe_uses_explicit_side_effect_free_mode() {
    let command = api_command(
        Path::new("D:/workspace"),
        Path::new("D:/workspace/api.exe"),
        RuntimeInputPaths::new(
            Path::new("D:/workspace/config"),
            Path::new("D:/workspace/locales"),
            &RuntimeSecrets::default(),
        ),
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
    assert_eq!(
        environment.get("APP_DATABASE_MIGRATION_MODE"),
        Some(&Some("verify".into()))
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
fn controlled_probe_stops_before_more_readiness_work_when_superseded() {
    let calls = RefCell::new(Vec::new());
    let control = wait_services_ready_until_controlled(
        Instant::now() + Duration::from_secs(1),
        "API",
        "Worker",
        || {
            calls.borrow_mut().push("ensure:api");
            Ok(())
        },
        || {
            calls.borrow_mut().push("ensure:worker");
            Ok(())
        },
        || {
            calls.borrow_mut().push("ready:api");
            false
        },
        || {
            calls.borrow_mut().push("ready:worker");
            false
        },
        || CycleControl::Superseded,
    )
    .unwrap();

    assert_eq!(control, CycleControl::Superseded);
    assert!(calls.into_inner().is_empty());
}

#[test]
fn controlled_probe_rechecks_between_api_and_worker_requests() {
    let calls = RefCell::new(Vec::new());
    let control_checks = AtomicUsize::new(0);
    let control = wait_services_ready_until_controlled(
        Instant::now() + Duration::from_secs(1),
        "API",
        "Worker",
        || {
            calls.borrow_mut().push("ensure:api");
            Ok(())
        },
        || {
            calls.borrow_mut().push("ensure:worker");
            Ok(())
        },
        || {
            calls.borrow_mut().push("ready:api");
            true
        },
        || -> bool { panic!("API 探活后的新代次应阻止 Worker 请求") },
        || {
            calls.borrow_mut().push("control");
            if control_checks.fetch_add(1, Ordering::Relaxed) == 0 {
                CycleControl::Continue
            } else {
                CycleControl::Superseded
            }
        },
    )
    .unwrap();

    assert_eq!(control, CycleControl::Superseded);
    assert_eq!(
        calls.into_inner(),
        [
            "control",
            "ensure:api",
            "ensure:worker",
            "ready:api",
            "control"
        ]
    );
}

#[test]
fn normal_services_start_without_probe_mode() {
    let api = api_command(
        Path::new("D:/workspace"),
        Path::new("D:/workspace/api.exe"),
        RuntimeInputPaths::new(
            Path::new("D:/workspace/config"),
            Path::new("D:/workspace/locales"),
            &RuntimeSecrets::default(),
        ),
        18080,
        19091,
        3,
        false,
    );
    let worker = worker_command(
        Path::new("D:/workspace"),
        Path::new("D:/workspace/worker.exe"),
        RuntimeInputPaths::new(
            Path::new("D:/workspace/config"),
            Path::new("D:/workspace/locales"),
            &RuntimeSecrets::default(),
        ),
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
fn invalid_config_probe_keeps_last_known_good_out_of_promotion() {
    let invalid_config = Err::<ProbeResult, _>("候选配置字段类型无效".into());
    let disposition = candidate_probe_disposition(&invalid_config);
    let mut active = "last-known-good";
    if disposition == CandidateProbeDisposition::Promote {
        active = "candidate";
    }

    assert_eq!(disposition, CandidateProbeDisposition::KeepLastKnownGood);
    assert_eq!(active, "last-known-good");
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
fn partial_stop_failure_restores_last_known_good_without_starting_candidate() {
    #[derive(Debug, PartialEq)]
    struct FakeServices {
        version: &'static str,
        api_running: bool,
        worker_running: bool,
    }

    let calls = RefCell::new(Vec::new());
    let mut current = FakeServices {
        version: "previous",
        api_running: true,
        worker_running: true,
    };
    let promoted = switch_services_with_rollback(
        &mut current,
        |services| {
            calls.borrow_mut().push("stop:api");
            services.api_running = false;
            Err("worker stop failed".into())
        },
        || panic!("部分停止失败时不能启动候选服务"),
        || {
            calls.borrow_mut().push("start:previous");
            Ok(FakeServices {
                version: "previous",
                api_running: true,
                worker_running: true,
            })
        },
        || panic!("部分停止失败时不能清理 last-known-good"),
    )
    .unwrap();

    assert!(!promoted);
    assert_eq!(
        current,
        FakeServices {
            version: "previous",
            api_running: true,
            worker_running: true,
        }
    );
    assert_eq!(calls.into_inner(), ["stop:api", "start:previous"]);
}

#[test]
fn stop_and_restore_failures_are_reported_together() {
    let mut current = "previous";
    let error = switch_services_with_rollback(
        &mut current,
        |_| Err("worker stop failed".into()),
        || panic!("停止失败时不能启动候选服务"),
        || Err("previous restore failed".into()),
        || panic!("停止失败时不能清理 last-known-good"),
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("worker stop failed"));
    assert!(error.contains("previous restore failed"));
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

#[cfg(windows)]
fn write_slow_migration_stub(root: &Path) -> PathBuf {
    let path = root.join("slow-migrate.cmd");
    fs::write(&path, "@echo off\r\nping -n 31 127.0.0.1 >NUL\r\n").unwrap();
    path
}

#[cfg(windows)]
fn write_blocking_cargo_stub(root: &Path) -> PathBuf {
    let path = root.join("fake-cargo.cmd");
    fs::write(
        &path,
        "@echo off\r\n>\".fake-cargo-started\" echo started\r\n>\"config\\fake-change.toml\" echo [app]\r\n:wait\r\nping -n 2 127.0.0.1 >NUL\r\ngoto wait\r\n",
    )
    .unwrap();
    path
}

#[cfg(unix)]
fn write_slow_migration_stub(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = root.join("slow-migrate.sh");
    fs::write(&path, "#!/bin/sh\nsleep 30\n").unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

#[cfg(unix)]
fn write_blocking_cargo_stub(root: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;

    let path = root.join("fake-cargo.sh");
    fs::write(
        &path,
        "#!/bin/sh\n: > .fake-cargo-started\nprintf '[app]\\n' > config/fake-change.toml\nwhile :; do sleep 60; done\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&path, permissions).unwrap();
    path
}
