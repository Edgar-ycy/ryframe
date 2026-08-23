use std::sync::atomic::{AtomicUsize, Ordering};

use ryframe::boot::startup::{
    ApiRunMode, WorkerRunMode, effective_migration_mode, parse_api_run_mode, parse_worker_run_mode,
    provision_or_verify,
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
    let worker = include_str!("../src/bin/ryframe_worker.rs");

    assert!(api_main.contains("effective_migration_mode("));
    assert!(api_storage.contains("|| storage.ensure_bucket(bucket)"));
    assert!(api_storage.contains("|| storage.readiness_check(bucket)"));
    assert!(api_redis.contains("|| client.ensure_scope_ownership(&ownership_marker)"));
    assert!(api_redis.contains("|| client.verify_scope_ownership(&ownership_marker)"));
    assert!(worker.contains("process_startup::effective_migration_mode("));
    assert!(worker.contains("|| storage.ensure_bucket(bucket)"));
    assert!(worker.contains("|| storage.readiness_check(bucket)"));
    assert!(worker.contains("|| client.ensure_scope_ownership(&ownership_marker)"));
    assert!(worker.contains("|| client.verify_scope_ownership(&ownership_marker)"));
}
