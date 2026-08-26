use std::sync::atomic::{AtomicUsize, Ordering};

use ryframe::boot::startup::{
    ApiRunMode, WorkerRunMode, effective_migration_mode, parse_api_run_mode, parse_worker_run_mode,
    provision_or_verify, wait_for_task_groups_until,
};
use ryframe_config::MigrationMode;

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn probe_modes_are_explicit_and_keep_normal_modes() {
    let serve = parse_api_run_mode(&[]).unwrap();
    let api_probe = parse_api_run_mode(&arguments(&["--probe"])).unwrap();
    assert_eq!(serve, ApiRunMode::Serve);
    assert!(serve.starts_background_tasks());
    assert_eq!(api_probe, ApiRunMode::Probe);
    assert!(!api_probe.starts_background_tasks());
    assert!(parse_api_run_mode(&arguments(&["--probe", "extra"])).is_err());

    let continuous = parse_worker_run_mode(&[]).unwrap();
    let once = parse_worker_run_mode(&arguments(&["--once"])).unwrap();
    let worker_probe = parse_worker_run_mode(&arguments(&["--probe"])).unwrap();
    assert_eq!(continuous, WorkerRunMode::Continuous);
    assert!(continuous.allows_initialization_writes());
    assert_eq!(once, WorkerRunMode::Once);
    assert!(once.allows_initialization_writes());
    assert_eq!(worker_probe, WorkerRunMode::Probe);
    assert!(!worker_probe.allows_initialization_writes());
    assert!(parse_worker_run_mode(&arguments(&["--probe", "extra"])).is_err());
}

#[test]
fn probe_turns_auto_migration_into_read_only_verification() {
    assert_eq!(
        effective_migration_mode(false, MigrationMode::Auto),
        MigrationMode::Verify
    );
    assert_eq!(
        effective_migration_mode(false, MigrationMode::Verify),
        MigrationMode::Verify
    );
    assert_eq!(
        effective_migration_mode(false, MigrationMode::Off),
        MigrationMode::Off
    );
    assert_eq!(
        effective_migration_mode(true, MigrationMode::Auto),
        MigrationMode::Auto
    );
}

#[tokio::test]
async fn probe_never_calls_ensure_or_its_cleanup_path() {
    let ensure_calls = AtomicUsize::new(0);
    let cleanup_calls = AtomicUsize::new(0);
    let readiness_calls = AtomicUsize::new(0);

    let result: Result<&str, ()> = provision_or_verify(
        false,
        || async {
            ensure_calls.fetch_add(1, Ordering::Relaxed);
            cleanup_calls.fetch_add(1, Ordering::Relaxed);
            Ok("ensured")
        },
        || async {
            readiness_calls.fetch_add(1, Ordering::Relaxed);
            Ok("verified")
        },
    )
    .await;

    assert_eq!(result, Ok("verified"));
    assert_eq!(ensure_calls.load(Ordering::Relaxed), 0);
    assert_eq!(cleanup_calls.load(Ordering::Relaxed), 0);
    assert_eq!(readiness_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn normal_startup_keeps_ensure_and_cleanup_behavior() {
    let ensure_calls = AtomicUsize::new(0);
    let cleanup_calls = AtomicUsize::new(0);
    let readiness_calls = AtomicUsize::new(0);

    let result: Result<&str, ()> = provision_or_verify(
        true,
        || async {
            ensure_calls.fetch_add(1, Ordering::Relaxed);
            cleanup_calls.fetch_add(1, Ordering::Relaxed);
            Ok("ensured")
        },
        || async {
            readiness_calls.fetch_add(1, Ordering::Relaxed);
            Ok("verified")
        },
    )
    .await;

    assert_eq!(result, Ok("ensured"));
    assert_eq!(ensure_calls.load(Ordering::Relaxed), 1);
    assert_eq!(cleanup_calls.load(Ordering::Relaxed), 1);
    assert_eq!(readiness_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn api_and_worker_wire_probe_to_read_only_dependency_checks() {
    let api_storage = include_str!("../src/boot/storage.rs");
    let api_redis = include_str!("../src/boot/redis.rs");
    let api_main = include_str!("../src/main.rs");
    let worker_main = include_str!("../src/bin/ryframe_worker.rs");
    let worker_runtime = include_str!("../src/bin/ryframe_worker/runtime.rs");

    assert!(api_main.contains("control_plane::prepare("));
    assert!(api_storage.contains("|| storage.ensure_bucket(bucket)"));
    assert!(api_storage.contains("|| storage.readiness_check(bucket)"));
    assert!(api_redis.contains("|| client.ensure_scope_ownership(&ownership_marker)"));
    assert!(api_redis.contains("|| client.verify_scope_ownership(&ownership_marker)"));
    assert!(worker_main.contains("control_plane::prepare("));
    assert!(worker_runtime.contains("|| storage.ensure_bucket(bucket)"));
    assert!(worker_runtime.contains("|| storage.readiness_check(bucket)"));
    assert!(worker_runtime.contains("|| client.ensure_scope_ownership(&ownership_marker)"));
    assert!(worker_runtime.contains("|| client.verify_scope_ownership(&ownership_marker)"));
}

#[test]
fn api_and_worker_share_control_plane_preparation() {
    let shared = include_str!("../src/boot/control_plane.rs");
    let api_main = include_str!("../src/main.rs");
    let worker_main = include_str!("../src/bin/ryframe_worker.rs");

    for operation in [
        "apply_migration(",
        "verify_schema(",
        "tenant_data::build_router(",
        "tenant_data::verify_current_targets(",
        "verify_fixed_tenant(",
    ] {
        assert!(shared.contains(operation));
        assert!(!api_main.contains(operation));
        assert!(!worker_main.contains(operation));
    }
    assert!(api_main.contains("ControlPlaneStartup::api("));
    assert!(worker_main.contains("ControlPlaneStartup::worker("));
}

#[test]
fn api_and_worker_share_background_service_builder() {
    let api_services = include_str!("../src/boot/services.rs");
    let shared_services = include_str!("../src/boot/background_services.rs");
    let worker_main = include_str!("../src/bin/ryframe_worker.rs");

    assert!(api_services.contains("build_background_services("));
    assert!(worker_main.contains("build_background_services("));
    for constructor in [
        "UserService::new",
        "ExportService::new",
        "TenantConfigTransferService::new",
        "TenantDataMigrationService::new",
    ] {
        assert!(shared_services.contains(constructor));
        assert!(!worker_main.contains(constructor));
    }
}

#[test]
fn background_job_handlers_are_grouped_by_domain() {
    let jobs = include_str!("../src/boot/jobs.rs");
    let groups = include_str!("../src/boot/jobs/handlers/mod.rs");
    let api_main = include_str!("../src/main.rs");
    let worker_main = include_str!("../src/bin/ryframe_worker.rs");

    assert!(jobs.contains("handlers::built_in(&dependencies)"));
    assert!(jobs.contains("with_handlers(built_in_handlers)"));
    for concrete_handler in [
        "ExportJobHandler::new",
        "DataRetentionJobHandler::new",
        "TenantConfigExportJobHandler::new",
        "MessageDispatchJobHandler::new",
    ] {
        assert!(!jobs.contains(concrete_handler));
    }
    for domain in ["exports", "messages", "retention", "tenant", "users"] {
        assert!(groups.contains(&format!("handlers.extend({domain}::handlers")));
    }
    assert!(api_main.contains("JobWorkerDependencies::from_api_services("));
    assert!(worker_main.contains("JobWorkerDependencies::from_background_services("));
    assert!(!api_main.contains("JobWorkerDependencies {"));
    assert!(!worker_main.contains("JobWorkerDependencies {"));
}

#[tokio::test]
async fn worker_and_health_tasks_share_expired_shutdown_deadline() {
    let mut worker_tasks = vec![tokio::spawn(std::future::pending::<()>())];
    let mut health_tasks = vec![tokio::spawn(std::future::pending::<()>())];
    let shared_deadline = tokio::time::Instant::now() - std::time::Duration::from_secs(1);

    wait_for_task_groups_until(
        &mut worker_tasks,
        "后台任务 Worker",
        &mut health_tasks,
        "Worker 健康服务",
        shared_deadline,
    )
    .await;

    let worker = worker_tasks.pop().expect("应保留 Worker 任务句柄");
    let health = health_tasks.pop().expect("应保留健康任务句柄");
    assert!(
        worker
            .await
            .expect_err("Worker 任务应被中止")
            .is_cancelled()
    );
    assert!(health.await.expect_err("健康任务应被中止").is_cancelled());
}
