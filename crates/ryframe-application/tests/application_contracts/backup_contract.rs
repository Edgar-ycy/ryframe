use super::backup_support::*;
use chrono::Duration;
use ryframe_application::{
    ports::backup::*,
    system::operations::{validate_backup_manifest, validate_restore_plan},
};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn registration_and_restore_success_have_separate_evidence_and_timing() {
    let (service, repository, _) = fixture();
    let manifest = manifest();
    let record = service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    assert!(record.valid);
    assert!(repository.records.lock().await.restores.is_empty());
    let running = service.begin_restore(plan()).await.unwrap();
    assert_eq!(running.status, RestoreStatus::Running);
    assert!(running.completed_at.is_none());
    repository.clock.fetch_add(600, Ordering::SeqCst);
    let repeated = service.begin_restore(plan()).await.unwrap();
    assert_eq!(repeated.started_at, running.started_at);
    let verified = service.verify_data(&running.plan.id).await.unwrap();
    assert_eq!(verified.status, RestoreStatus::DataVerified);
    let completed = service
        .finish_restore(&running.plan.id, &proof(&verified))
        .await
        .unwrap();
    assert_eq!(completed.status, RestoreStatus::Succeeded);
    assert_eq!(
        (completed.completed_at.unwrap() - completed.started_at).num_seconds(),
        600
    );
    assert_eq!(completed.recovered_at, manifest.captured_at);
}

#[tokio::test]
async fn corrupt_or_missing_files_are_persisted_as_invalid_and_cannot_begin_restore() {
    let (service, repository, verifier) = fixture();
    verifier.broken_file.store(true, Ordering::SeqCst);
    let manifest = manifest();
    assert!(
        service
            .register(manifest.clone(), &manifest.resource_keys())
            .await
            .is_err()
    );
    let record = repository.backup(&manifest.id).await.unwrap().unwrap();
    assert!(!record.valid);
    assert!(record.failure.is_some());
    assert!(service.begin_restore(plan()).await.is_err());
}

#[tokio::test]
async fn damaged_restored_data_records_failed_drill() {
    let (service, repository, verifier) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    service.begin_restore(plan()).await.unwrap();
    verifier.broken_data.store(true, Ordering::SeqCst);
    assert!(service.verify_data("drill-1").await.is_err());
    let failed = repository.restore("drill-1").await.unwrap().unwrap();
    assert_eq!(failed.status, RestoreStatus::Failed);
    assert!(failed.completed_at.is_some());
    assert!(failed.data_verified_at.is_none());
}

#[tokio::test]
async fn runtime_probe_time_counts_towards_the_one_hour_limit() {
    let (service, repository, verifier) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    service.begin_restore(plan()).await.unwrap();
    let verified = service.verify_data("drill-1").await.unwrap();
    verifier.runtime_delay_seconds.store(3601, Ordering::SeqCst);
    assert!(
        service
            .finish_restore("drill-1", &proof(&verified))
            .await
            .is_err()
    );
    assert_eq!(
        repository.restore("drill-1").await.unwrap().unwrap().status,
        RestoreStatus::Failed
    );
}

#[tokio::test]
async fn stale_or_incomplete_business_evidence_never_marks_recovery_successful() {
    for case in 0..7 {
        let (service, repository, _) = fixture();
        let manifest = manifest();
        service
            .register(manifest.clone(), &manifest.resource_keys())
            .await
            .unwrap();
        service.begin_restore(plan()).await.unwrap();
        let verified = service.verify_data("drill-1").await.unwrap();
        let mut proof = proof(&verified);
        match case {
            0 => {
                proof.scenarios.pop();
            }
            1 => {
                proof.frontend_sha = "f".repeat(40);
            }
            2 => {
                proof.plan_hash = "0".repeat(64);
            }
            3 => {
                proof.unexpected_network_failures = 1;
            }
            4 => {
                proof.axe_serious_or_critical = 1;
            }
            5 => {
                proof.scenarios[0].succeeded = false;
            }
            _ => {
                proof.runtime_receipt_sha256.clear();
            }
        }
        assert!(service.finish_restore("drill-1", &proof).await.is_err());
        assert_eq!(
            repository.restore("drill-1").await.unwrap().unwrap().status,
            RestoreStatus::Failed
        );
    }
}

#[tokio::test]
async fn identifiers_cannot_replace_existing_backup_or_restore_plans() {
    let (service, repository, _) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    let mut changed = manifest.clone();
    changed.source_sha = "f".repeat(40);
    assert!(
        service
            .register(changed, &manifest.resource_keys())
            .await
            .is_err()
    );
    assert_eq!(
        repository
            .backup(&manifest.id)
            .await
            .unwrap()
            .unwrap()
            .manifest,
        manifest
    );
    let original = service.begin_restore(plan()).await.unwrap();
    let mut changed = plan();
    changed.object_endpoint = "elsewhere:9000".into();
    assert!(service.begin_restore(changed).await.is_err());
    assert_eq!(
        repository.restore("drill-1").await.unwrap().unwrap().plan,
        original.plan
    );
}

#[tokio::test]
async fn ranges_timestamps_and_source_resource_reuse_fail_closed() {
    let (_, repository, _) = fixture();
    let now = repository.database_now().await.unwrap();
    let original = manifest();
    for case in 0..6 {
        let mut manifest = original.clone();
        match case {
            0 => {
                manifest.artifacts.pop();
            }
            1 => {
                manifest.artifacts[0].relative_path = "../outside.sql".into();
            }
            2 => {
                manifest.completed_at = now + Duration::seconds(1);
            }
            3 => {
                manifest.retention_until = now + Duration::hours(1);
            }
            4 => {
                manifest.objects[0].prefix = "other/".into();
            }
            _ => {
                manifest.databases.push(manifest.databases[0].clone());
            }
        }
        assert!(validate_backup_manifest(&manifest, &original.resource_keys(), now).is_err());
    }
    let backup = BackupRecord {
        manifest: original,
        manifest_hash: "e".repeat(64),
        valid: true,
        checked_at: now,
        failure: None,
    };
    let mut same_database = plan();
    same_database.databases[0].database = "source_db".into();
    assert!(validate_restore_plan(&backup, &same_database, now).is_err());
    let mut old = backup;
    old.manifest.captured_at = now - Duration::hours(25);
    assert!(validate_restore_plan(&old, &plan(), now).is_err());
}
