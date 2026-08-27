use std::{
    cell::RefCell,
    path::Path,
    time::{Duration, Instant},
};

use super::dev::{
    ArtifactAction, BuildPlan, ChangeKind, CycleControl, RuntimeInputPaths, api_command,
    available_ports, classify_change, combine_failures, start_worker_after_api_ready,
    switch_services_with_rollback, wait_services_ready_until, wait_services_ready_until_controlled,
    worker_command,
};
use super::watch::{ChangeBatch, SourceRevision};

#[test]
fn development_changes_are_classified_by_runtime_impact() {
    for (path, expected) in [
        ("config/app.dev.toml", ChangeKind::RuntimeConfig),
        ("config/app.prod.toml", ChangeKind::IgnoredDevConfig),
        ("locales/zh-CN.toml", ChangeKind::LocaleCatalog),
        ("crates/ryframe-api/src/lib.rs", ChangeKind::ApiOnly),
        (
            "crates/ryframe/src/bin/ryframe_worker.rs",
            ChangeKind::WorkerOnly,
        ),
        (
            "crates/ryframe-application/src/lib.rs",
            ChangeKind::SharedRuntime,
        ),
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
        ("Cargo.lock", ChangeKind::BuildGraph),
    ] {
        assert_eq!(classify_change(path), expected, "错误分类 {path}");
    }
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
fn normal_services_start_without_probe_mode() {
    let api = api_command(
        Path::new("D:/workspace"),
        Path::new("D:/workspace/api.exe"),
        RuntimeInputPaths::new(
            Path::new("D:/workspace/config"),
            Path::new("D:/workspace/locales"),
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
