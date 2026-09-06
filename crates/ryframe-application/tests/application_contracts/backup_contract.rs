use super::backup_support::*;
use chrono::Duration;
use ryframe_application::{
    ports::backup::*,
    system::operations::{validate_backup_manifest, validate_restore_plan},
};
use ryframe_kernel::{AppError, AppResult};
use std::sync::atomic::Ordering;

fn data_verified_record(record: &RestoreRecord) -> RestoreRecord {
    let mut next = record.clone();
    next.status = RestoreStatus::DataVerified;
    next.data_verified_at = Some(record.started_at + Duration::seconds(1));
    next
}

async fn persist_restore_advance(
    repository: &Repository,
    expected: &RestoreRecord,
    next: &RestoreRecord,
) -> AppResult<RestoreRecord> {
    let transaction = repository.begin().await?;
    let record = transaction.advance_restore(expected, next).await?;
    transaction.commit().await?;
    Ok(record)
}

fn assert_restore_identity(actual: &RestoreRecord, expected: &RestoreRecord) {
    assert_eq!(actual.plan, expected.plan);
    assert_eq!(actual.plan_hash, expected.plan_hash);
    assert_eq!(actual.started_at, expected.started_at);
    assert_eq!(actual.recovered_at, expected.recovered_at);
}

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
    assert_eq!(repeated, running);
    let verified = service.verify_data(&running.plan.id).await.unwrap();
    assert_eq!(verified.status, RestoreStatus::DataVerified);
    assert_restore_identity(&verified, &running);
    let completed = service
        .finish_restore(&running.plan.id, &proof(&verified))
        .await
        .unwrap();
    assert_eq!(completed.status, RestoreStatus::Succeeded);
    assert_restore_identity(&completed, &running);
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
    let error = service.verify_data("drill-1").await.unwrap_err();
    assert!(error.message().contains("注入的校验失败"));
    let failed = repository.restore("drill-1").await.unwrap().unwrap();
    assert_eq!(failed.status, RestoreStatus::Failed);
    assert!(failed.completed_at.is_some());
    assert!(failed.data_verified_at.is_none());
}

#[tokio::test]
async fn create_restore_retry_returns_the_first_authoritative_record() {
    let (service, repository, _) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    let original = service.begin_restore(plan()).await.unwrap();
    let mut retry = original.clone();
    retry.started_at += Duration::minutes(5);
    retry.recovered_at += Duration::minutes(5);

    let transaction = repository.begin().await.unwrap();
    let authoritative = transaction.create_restore(&retry).await.unwrap();
    transaction.commit().await.unwrap();

    assert_eq!(authoritative, original);
    let mut conflicting = original.clone();
    conflicting.plan_hash = "f".repeat(64);
    let transaction = repository.begin().await.unwrap();
    assert!(matches!(
        transaction.create_restore(&conflicting).await,
        Err(AppError::Conflict(_))
    ));
    drop(transaction);
    assert_eq!(
        repository.restore("drill-1").await.unwrap().unwrap(),
        original
    );
}

#[tokio::test]
async fn advance_restore_rejects_immutable_changes_and_illegal_transitions() {
    let (service, repository, _) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    let running = service.begin_restore(plan()).await.unwrap();

    let mut candidates = Vec::new();
    let mut changed = data_verified_record(&running);
    changed.plan.scope_id = "other-target".into();
    candidates.push(changed);
    let mut changed = data_verified_record(&running);
    changed.plan_hash = "f".repeat(64);
    candidates.push(changed);
    let mut changed = data_verified_record(&running);
    changed.started_at += Duration::seconds(1);
    candidates.push(changed);
    let mut changed = data_verified_record(&running);
    changed.recovered_at += Duration::seconds(1);
    candidates.push(changed);
    let mut illegal = running.clone();
    illegal.status = RestoreStatus::Succeeded;
    illegal.completed_at = Some(running.started_at + Duration::seconds(1));
    candidates.push(illegal);

    for candidate in candidates {
        assert!(matches!(
            persist_restore_advance(repository.as_ref(), &running, &candidate).await,
            Err(AppError::Conflict(_))
        ));
    }
    assert_eq!(
        repository.restore("drill-1").await.unwrap().unwrap(),
        running
    );
}

#[tokio::test]
async fn persistence_contract_rejects_malformed_restore_record_shapes() {
    let (service, repository, _) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    let running = service.begin_restore(plan()).await.unwrap();

    let mut invalid_creates = Vec::new();
    let mut invalid = running.clone();
    invalid.plan.id = "restore-invalid-hash".into();
    invalid.plan_hash = "f".repeat(64);
    invalid_creates.push(invalid);
    let mut invalid = running.clone();
    invalid.plan.id = "restore-invalid-initial-state".into();
    invalid.plan_hash = backup_content_hash(&invalid.plan).unwrap();
    invalid.status = RestoreStatus::Failed;
    invalid.completed_at = Some(running.started_at);
    invalid.failure = Some("错误初始状态".into());
    invalid_creates.push(invalid);
    for invalid in invalid_creates {
        let transaction = repository.begin().await.unwrap();
        assert!(matches!(
            transaction.create_restore(&invalid).await,
            Err(AppError::Validation(_))
        ));
    }

    let mut malformed = Vec::new();
    let mut next = data_verified_record(&running);
    next.data_verified_at = None;
    malformed.push(next);
    let mut next = data_verified_record(&running);
    next.completed_at = next.data_verified_at;
    malformed.push(next);
    let mut next = data_verified_record(&running);
    next.failure = Some("数据已验证状态不能带失败".into());
    malformed.push(next);
    let mut next = data_verified_record(&running);
    next.data_verified_at =
        Some(running.started_at + Duration::hours(1) + Duration::nanoseconds(1));
    malformed.push(next);
    let mut next = running.clone();
    next.status = RestoreStatus::Failed;
    next.data_verified_at = Some(running.started_at + Duration::seconds(1));
    next.completed_at = next.data_verified_at;
    next.failure = Some("数据验证前失败".into());
    malformed.push(next);
    for next in malformed {
        assert!(matches!(
            persist_restore_advance(repository.as_ref(), &running, &next).await,
            Err(AppError::Validation(_))
        ));
    }

    let verified = data_verified_record(&running);
    persist_restore_advance(repository.as_ref(), &running, &verified)
        .await
        .unwrap();
    let mut changed_verified_at = verified.clone();
    changed_verified_at.status = RestoreStatus::Succeeded;
    changed_verified_at.data_verified_at =
        Some(verified.data_verified_at.unwrap() + Duration::seconds(1));
    changed_verified_at.completed_at = changed_verified_at.data_verified_at;
    assert!(matches!(
        persist_restore_advance(repository.as_ref(), &verified, &changed_verified_at).await,
        Err(AppError::Conflict(_))
    ));
    let mut malformed = Vec::new();
    let mut next = verified.clone();
    next.status = RestoreStatus::Succeeded;
    malformed.push(next);
    let mut next = verified.clone();
    next.status = RestoreStatus::Succeeded;
    next.completed_at = Some(verified.data_verified_at.unwrap() - Duration::nanoseconds(1));
    malformed.push(next);
    let mut next = verified.clone();
    next.status = RestoreStatus::Succeeded;
    next.completed_at = Some(verified.data_verified_at.unwrap());
    next.failure = Some("成功状态不能带失败".into());
    malformed.push(next);
    let mut next = verified.clone();
    next.status = RestoreStatus::Failed;
    next.completed_at = Some(verified.data_verified_at.unwrap());
    next.failure = Some(String::new());
    malformed.push(next);
    let mut next = verified.clone();
    next.status = RestoreStatus::Failed;
    next.completed_at = verified.data_verified_at;
    next.failure = Some("   ".into());
    malformed.push(next);
    for next in malformed {
        assert!(matches!(
            persist_restore_advance(repository.as_ref(), &verified, &next).await,
            Err(AppError::Validation(_))
        ));
    }
}

#[tokio::test]
async fn corrupted_authoritative_records_fail_before_external_verification() {
    let (service, repository, verifier) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    let running = service.begin_restore(plan()).await.unwrap();

    let mut malformed_existing = running.clone();
    malformed_existing.status = RestoreStatus::Succeeded;
    repository
        .records
        .lock()
        .await
        .restores
        .insert(running.plan.id.clone(), malformed_existing);
    assert!(matches!(
        service.begin_restore(plan()).await,
        Err(AppError::Validation(_))
    ));

    let mut malformed_running = running.clone();
    malformed_running.plan_hash = "f".repeat(64);
    repository
        .records
        .lock()
        .await
        .restores
        .insert(running.plan.id.clone(), malformed_running);
    verifier.artifact_calls.store(0, Ordering::SeqCst);
    verifier.restored_database_calls.store(0, Ordering::SeqCst);
    verifier.restored_object_calls.store(0, Ordering::SeqCst);
    assert!(matches!(
        service.verify_data(&running.plan.id).await,
        Err(AppError::Validation(_))
    ));
    assert_eq!(verifier.artifact_calls.load(Ordering::SeqCst), 0);
    assert_eq!(verifier.restored_database_calls.load(Ordering::SeqCst), 0);
    assert_eq!(verifier.restored_object_calls.load(Ordering::SeqCst), 0);

    let verified = data_verified_record(&running);
    let mut malformed_verified = verified.clone();
    malformed_verified.completed_at = malformed_verified.data_verified_at;
    repository
        .records
        .lock()
        .await
        .restores
        .insert(running.plan.id.clone(), malformed_verified);
    verifier.runtime_calls.store(0, Ordering::SeqCst);
    assert!(matches!(
        service
            .finish_restore(&running.plan.id, &proof(&verified))
            .await,
        Err(AppError::Validation(_))
    ));
    assert_eq!(verifier.runtime_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn clock_rollback_after_data_verification_is_persisted_as_failure() {
    let (service, repository, _) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    let running = service.begin_restore(plan()).await.unwrap();
    repository.clock.fetch_add(10, Ordering::SeqCst);
    let verified = service.verify_data(&running.plan.id).await.unwrap();
    repository.clock.fetch_sub(5, Ordering::SeqCst);

    let error = service
        .finish_restore(&running.plan.id, &proof(&verified))
        .await
        .unwrap_err();
    assert!(error.message().contains("时钟发生回退"));
    let failed = repository.restore(&running.plan.id).await.unwrap().unwrap();
    assert_eq!(failed.status, RestoreStatus::Failed);
    assert_eq!(failed.completed_at, failed.data_verified_at);
}

#[tokio::test]
async fn concurrent_restore_advances_allow_exactly_one_cas_winner() {
    let (service, repository, _) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    let running = service.begin_restore(plan()).await.unwrap();
    let next = data_verified_record(&running);

    let first = persist_restore_advance(repository.as_ref(), &running, &next);
    let second = persist_restore_advance(repository.as_ref(), &running, &next);
    let (first, second) = tokio::join!(first, second);

    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        usize::from(matches!(first, Err(AppError::Conflict(_))))
            + usize::from(matches!(second, Err(AppError::Conflict(_)))),
        1
    );
    assert_eq!(repository.restore("drill-1").await.unwrap().unwrap(), next);
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
    for case in 0..8 {
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
            6 => {
                proof.runtime_receipt_sha256.clear();
            }
            _ => proof.scenarios.push(RestoreScenarioResult {
                name: "unknown-extra-scenario".into(),
                succeeded: true,
            }),
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
