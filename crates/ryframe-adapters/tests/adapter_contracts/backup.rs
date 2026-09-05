use chrono::{Duration, Utc};
use ryframe_adapters::{
    backup::{artifact_verifier, object_verifier},
    storage::{LocalObjectStorage, ObjectStorage, ScopedObjectStorage},
};
use ryframe_application::{ports::backup::*, system::operations::BACKUP_OBJECT_BUCKETS};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn manifest() -> BackupManifest {
    BackupManifest {
        id: "backup".into(),
        scope_id: "source".into(),
        source_sha: "a".repeat(40),
        quiesced_at: Utc::now(),
        captured_at: Utc::now(),
        completed_at: Utc::now(),
        retention_until: Utc::now() + Duration::days(7),
        control_schema_fingerprint: "a".repeat(16),
        tenant_schema_fingerprint: "b".repeat(64),
        databases: vec![],
        objects: vec![],
        artifacts: vec![],
    }
}

#[tokio::test]
async fn artifact_checks_fail_for_missing_corrupt_and_escaped_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("backup");
    std::fs::create_dir(&root).unwrap();
    let path = root.join("control.sql");
    std::fs::write(&path, b"verified backup").unwrap();
    let mut manifest = manifest();
    manifest.artifacts.push(BackupArtifact {
        resource: "db:shared-control".into(),
        relative_path: "control.sql".into(),
        bytes: 15,
        sha256: hex::encode(Sha256::digest(b"verified backup")),
    });
    let verifier = artifact_verifier(root.clone());
    verifier.artifacts(&manifest).await.unwrap();
    std::fs::write(&path, b"corrupt backup!").unwrap();
    assert!(verifier.artifacts(&manifest).await.is_err());
    manifest.artifacts[0].relative_path = "missing.sql".into();
    assert!(verifier.artifacts(&manifest).await.is_err());
    std::fs::write(directory.path().join("outside.sql"), b"verified backup").unwrap();
    manifest.artifacts[0].relative_path = "../outside.sql".into();
    assert!(verifier.artifacts(&manifest).await.is_err());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn artifact_checks_reject_linked_roots_and_path_components() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("backup");
    let outside = directory.path().join("outside");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("control.sql"), b"verified backup").unwrap();

    let linked = root.join("linked");
    if let Err(error) = create_directory_link(&outside, &linked) {
        if link_creation_is_unavailable(&error) {
            eprintln!("SKIP: 当前平台不允许创建测试用目录链接: {error}");
            return;
        }
        panic!("创建测试用目录链接失败: {error}");
    }
    let mut value = manifest();
    value.artifacts.push(BackupArtifact {
        resource: "db:shared-control".into(),
        relative_path: "linked/control.sql".into(),
        bytes: 15,
        sha256: hex::encode(Sha256::digest(b"verified backup")),
    });
    assert!(
        artifact_verifier(root.clone())
            .artifacts(&value)
            .await
            .is_err()
    );

    let linked_root = directory.path().join("linked-root");
    create_directory_link(&root, &linked_root).unwrap();
    value.artifacts[0].relative_path = "control.sql".into();
    std::fs::write(root.join("control.sql"), b"verified backup").unwrap();
    assert!(
        artifact_verifier(linked_root)
            .artifacts(&value)
            .await
            .is_err()
    );
}

#[cfg(unix)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(any(unix, windows))]
fn link_creation_is_unavailable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
    ) || error.raw_os_error() == Some(1314)
}

async fn storage(scope: &str, root: &std::path::Path) -> Arc<dyn ObjectStorage> {
    let storage: Arc<dyn ObjectStorage> = Arc::new(LocalObjectStorage::new(root));
    for bucket in BACKUP_OBJECT_BUCKETS {
        storage
            .put(
                bucket,
                &format!("{scope}/.ryframe-owner"),
                format!("ryframe-owner:v1:{scope}:object-storage:{bucket}").as_bytes(),
                "text/plain",
            )
            .await
            .unwrap();
    }
    storage
}

#[tokio::test]
async fn object_restore_requires_owned_complete_inventory_and_matching_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let source = storage("source", directory.path()).await;
    let restored = storage("restored", directory.path()).await;
    let bytes = vec![42_u8; 2 * 1024 * 1024 + 7];
    source
        .put(
            "exports",
            "source/data.bin",
            &bytes,
            "application/octet-stream",
        )
        .await
        .unwrap();
    restored
        .put(
            "exports",
            "restored/data.bin",
            &bytes,
            "application/octet-stream",
        )
        .await
        .unwrap();
    let source_verifier = object_verifier(source, "local".into(), "source".into());
    let restore_verifier = object_verifier(restored.clone(), "local".into(), "restored".into());
    let mut manifest = manifest();
    manifest.objects = source_verifier.snapshot().await.unwrap();
    assert_eq!(
        manifest
            .objects
            .iter()
            .map(|bucket| bucket.entries.len())
            .sum::<usize>(),
        1
    );
    let plan = restore_plan();
    restore_verifier
        .validate_restore_targets(&plan)
        .await
        .unwrap();
    restore_verifier
        .restored_objects(&manifest, &plan)
        .await
        .unwrap();
    assert_invalid_restore(
        restored.as_ref(),
        restore_verifier.as_ref(),
        &manifest,
        &plan,
    )
    .await;
}

#[tokio::test]
async fn scoped_storage_digest_uses_the_scoped_physical_key() {
    let directory = tempfile::tempdir().unwrap();
    let inner: Arc<dyn ObjectStorage> = Arc::new(LocalObjectStorage::new(directory.path()));
    let scoped = ScopedObjectStorage::new(inner, "source");
    scoped
        .put(
            "exports",
            "data.bin",
            b"scoped bytes",
            "application/octet-stream",
        )
        .await
        .unwrap();
    let digest = scoped.digest("exports", "data.bin").await.unwrap();
    assert_eq!(digest.bytes, 12);
    assert_eq!(digest.sha256, hex::encode(Sha256::digest(b"scoped bytes")));
    assert!(scoped.digest("exports", "source/data.bin").await.is_err());
}

#[cfg(feature = "monitoring")]
#[test]
fn backup_metrics_have_capture_time_and_only_fixed_labels() {
    let captured = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    ryframe_adapters::metrics::set_backup_health(
        &BackupHealth {
            required_resources: 6,
            missing_resources: 1,
            expired_resources: 2,
            invalid_resources: 3,
            oldest_capture: Some(captured),
            last_restore_completed: Some(captured),
            last_restore_succeeded: true,
            restore_duration_seconds: Some(42),
            recovery_point_age_seconds: Some(86_400),
            restore_running: 1,
            restore_overdue: 2,
        },
        chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
    );
    let text = ryframe_adapters::metrics::metrics_text();
    assert!(text.contains("ryframe_backup_last_success_timestamp_seconds 1700000000"));
    assert!(text.contains("ryframe_backup_collector_last_success_timestamp_seconds 1800000000"));
    assert!(text.contains("ryframe_backup_resources{state=\"missing\"} 1"));
    assert!(text.contains("ryframe_restore_last_succeeded 1"));
    assert!(text.contains("ryframe_restore_duration_seconds 42"));
    assert!(text.contains("ryframe_restore_recovery_point_age_seconds 86400"));
    let series = text
        .lines()
        .filter(|line| line.starts_with("ryframe_backup_") || line.starts_with("ryframe_restore_"))
        .map(|line| line.split_whitespace().next().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    // 精确集合同时拒绝 tenant、scope、endpoint、object、object_key、target 等额外标签。
    assert_eq!(
        series,
        [
            "ryframe_backup_collector_up",
            "ryframe_backup_collector_last_success_timestamp_seconds",
            "ryframe_backup_last_success_timestamp_seconds",
            "ryframe_backup_resources{state=\"expired\"}",
            "ryframe_backup_resources{state=\"invalid\"}",
            "ryframe_backup_resources{state=\"missing\"}",
            "ryframe_backup_resources{state=\"required\"}",
            "ryframe_restore_duration_seconds",
            "ryframe_restore_last_completed_timestamp_seconds",
            "ryframe_restore_last_succeeded",
            "ryframe_restore_recovery_point_age_seconds",
            "ryframe_restore_runs{state=\"overdue\"}",
            "ryframe_restore_runs{state=\"running\"}",
        ]
        .into_iter()
        .collect()
    );
    ryframe_adapters::metrics::set_backup_collector_failed();
    let failed = ryframe_adapters::metrics::metrics_text();
    assert!(failed.contains("ryframe_backup_collector_up 0"));
    assert!(failed.contains("ryframe_backup_collector_last_success_timestamp_seconds 1800000000"));
}

fn restore_plan() -> RestorePlan {
    RestorePlan {
        id: "drill".into(),
        backup_id: "backup".into(),
        scope_id: "restored".into(),
        fault_at: Utc::now(),
        databases: vec![],
        object_endpoint: "local".into(),
        object_prefix: "restored/".into(),
        api_ready_url: "http://localhost/readyz".into(),
        worker_ready_url: "http://localhost:9090/readyz".into(),
        frontend_sha: "c".repeat(40),
    }
}

async fn assert_invalid_restore(
    restored: &dyn ObjectStorage,
    restore_verifier: &dyn BackupObjectVerifier,
    manifest: &BackupManifest,
    plan: &RestorePlan,
) {
    restored
        .put(
            "exports",
            "restored/unexpected.bin",
            b"extra",
            "application/octet-stream",
        )
        .await
        .unwrap();
    assert!(
        restore_verifier
            .restored_objects(manifest, plan)
            .await
            .is_err()
    );
    restored
        .delete("exports", "restored/unexpected.bin")
        .await
        .unwrap();
    restored
        .put(
            "exports",
            "restored/data.bin",
            b"wrong",
            "application/octet-stream",
        )
        .await
        .unwrap();
    assert!(
        restore_verifier
            .restored_objects(manifest, plan)
            .await
            .is_err()
    );
    restored
        .delete("exports", "restored/data.bin")
        .await
        .unwrap();
    assert!(
        restore_verifier
            .restored_objects(manifest, plan)
            .await
            .is_err()
    );
    restored
        .put(
            "exports",
            "restored/.ryframe-owner",
            b"foreign owner",
            "text/plain",
        )
        .await
        .unwrap();
    assert!(
        restore_verifier
            .validate_restore_targets(plan)
            .await
            .is_err()
    );
}
