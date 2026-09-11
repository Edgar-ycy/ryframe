use super::backup_support::*;
use chrono::Duration;
use ryframe_application::{
    ports::backup::*,
    system::operations::{validate_backup_manifest, validate_backup_record, validate_restore_plan},
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
async fn registration_keeps_the_newest_authoritative_verification() {
    let (service, repository, verifier) = fixture();
    let manifest = manifest();
    let required = manifest.resource_keys();
    service.register(manifest.clone(), &required).await.unwrap();

    verifier.broken_file.store(true, Ordering::SeqCst);
    let error = service
        .register(manifest.clone(), &required)
        .await
        .unwrap_err();
    assert!(error.message().contains("注入的校验失败"));
    assert!(
        repository
            .backup(&manifest.id)
            .await
            .unwrap()
            .unwrap()
            .valid
    );

    repository.clock.fetch_add(10, Ordering::SeqCst);
    assert!(service.register(manifest.clone(), &required).await.is_err());
    let invalid = repository.backup(&manifest.id).await.unwrap().unwrap();
    assert!(!invalid.valid);
    assert!(invalid.failure.is_some());

    repository.clock.fetch_sub(5, Ordering::SeqCst);
    verifier.broken_file.store(false, Ordering::SeqCst);
    assert!(service.register(manifest.clone(), &required).await.is_err());
    assert_eq!(
        repository.backup(&manifest.id).await.unwrap(),
        Some(invalid)
    );

    repository.clock.fetch_add(10, Ordering::SeqCst);
    let valid = service.register(manifest, &required).await.unwrap();
    assert!(valid.valid);
    assert!(valid.failure.is_none());

    repository.clock.fetch_sub(5, Ordering::SeqCst);
    verifier.broken_file.store(true, Ordering::SeqCst);
    let error = service
        .register(valid.manifest.clone(), &required)
        .await
        .unwrap_err();
    assert!(error.message().contains("注入的校验失败"));
    assert_eq!(
        repository.backup(&valid.manifest.id).await.unwrap(),
        Some(valid)
    );
}

#[tokio::test]
async fn registration_rechecks_retention_after_external_verification() {
    let (service, repository, verifier) = fixture();
    let manifest = manifest();
    verifier
        .artifact_delay_seconds
        .store(7 * 24 * 60 * 60 + 1, Ordering::SeqCst);

    assert!(matches!(
        service
            .register(manifest.clone(), &manifest.resource_keys())
            .await,
        Err(AppError::Validation(_))
    ));
    assert!(repository.backup(&manifest.id).await.unwrap().is_none());
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
async fn first_restore_create_revalidates_parent_and_recovery_point() {
    let (service, repository, _) = fixture();
    let manifest = manifest();
    service
        .register(manifest.clone(), &manifest.resource_keys())
        .await
        .unwrap();
    let running = service.begin_restore(plan()).await.unwrap();
    let backup = repository.backup(&manifest.id).await.unwrap().unwrap();

    let mut invalid = backup.clone();
    invalid.valid = false;
    invalid.failure = Some("重新校验失败".into());
    repository
        .records
        .lock()
        .await
        .backups
        .insert(manifest.id.clone(), invalid);
    let candidate = restore_with_id(&running, "restore-invalid-parent");
    let transaction = repository.begin().await.unwrap();
    let error = transaction.create_restore(&candidate).await.unwrap_err();
    assert!(error.message().contains("备份已失效或不在保留期内"));
    drop(transaction);

    repository
        .records
        .lock()
        .await
        .backups
        .insert(manifest.id.clone(), backup.clone());
    let original_now = repository.clock.load(Ordering::SeqCst);
    repository.clock.store(
        backup.manifest.retention_until.timestamp(),
        Ordering::SeqCst,
    );
    let candidate = restore_with_id(&running, "restore-expired-parent");
    let transaction = repository.begin().await.unwrap();
    let error = transaction.create_restore(&candidate).await.unwrap_err();
    assert!(error.message().contains("备份已失效或不在保留期内"));
    drop(transaction);
    repository.clock.store(original_now, Ordering::SeqCst);

    let mut candidate = restore_with_id(&running, "restore-wrong-recovery-point");
    candidate.recovered_at += Duration::microseconds(1);
    let transaction = repository.begin().await.unwrap();
    let error = transaction.create_restore(&candidate).await.unwrap_err();
    assert!(error.message().contains("实际恢复点与备份采集时间不一致"));
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

    for invalid in malformed_restore_creates(&running) {
        let transaction = repository.begin().await.unwrap();
        assert!(matches!(
            transaction.create_restore(&invalid).await,
            Err(AppError::Validation(_))
        ));
    }

    for next in malformed_running_advances(&running) {
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
    for next in malformed_verified_advances(&verified) {
        assert!(matches!(
            persist_restore_advance(repository.as_ref(), &verified, &next).await,
            Err(AppError::Validation(_))
        ));
    }
}

fn malformed_restore_creates(running: &RestoreRecord) -> Vec<RestoreRecord> {
    let mut invalid_hash = running.clone();
    invalid_hash.plan.id = "restore-invalid-hash".into();
    invalid_hash.plan_hash = "f".repeat(64);
    let mut invalid_state = running.clone();
    invalid_state.plan.id = "restore-invalid-initial-state".into();
    invalid_state.plan_hash = backup_content_hash(&invalid_state.plan).unwrap();
    invalid_state.status = RestoreStatus::Failed;
    invalid_state.completed_at = Some(running.started_at);
    invalid_state.failure = Some("错误初始状态".into());
    let mut invalid_start = running.clone();
    invalid_start.plan.id = "restore-nanosecond-time".into();
    invalid_start.plan_hash = backup_content_hash(&invalid_start.plan).unwrap();
    invalid_start.started_at += Duration::nanoseconds(1);
    let mut invalid_recovery = running.clone();
    invalid_recovery.plan.id = "restore-nanosecond-recovery".into();
    invalid_recovery.plan_hash = backup_content_hash(&invalid_recovery.plan).unwrap();
    invalid_recovery.recovered_at += Duration::nanoseconds(1);
    vec![invalid_hash, invalid_state, invalid_start, invalid_recovery]
}

fn restore_with_id(running: &RestoreRecord, id: &str) -> RestoreRecord {
    let mut candidate = running.clone();
    candidate.plan.id = id.into();
    candidate.plan_hash = backup_content_hash(&candidate.plan).unwrap();
    candidate
}

fn malformed_running_advances(running: &RestoreRecord) -> Vec<RestoreRecord> {
    let mut missing_time = data_verified_record(running);
    missing_time.data_verified_at = None;
    let mut imprecise_time = data_verified_record(running);
    imprecise_time.data_verified_at = imprecise_time
        .data_verified_at
        .map(|timestamp| timestamp + Duration::nanoseconds(1));
    let mut completed = data_verified_record(running);
    completed.completed_at = completed.data_verified_at;
    let mut failure = data_verified_record(running);
    failure.failure = Some("数据已验证状态不能带失败".into());
    let mut overdue = data_verified_record(running);
    overdue.data_verified_at =
        Some(running.started_at + Duration::hours(1) + Duration::nanoseconds(1));
    let mut failed_after_verification = running.clone();
    failed_after_verification.status = RestoreStatus::Failed;
    failed_after_verification.data_verified_at = Some(running.started_at + Duration::seconds(1));
    failed_after_verification.completed_at = failed_after_verification.data_verified_at;
    failed_after_verification.failure = Some("数据验证前失败".into());
    vec![
        missing_time,
        imprecise_time,
        completed,
        failure,
        overdue,
        failed_after_verification,
    ]
}

fn malformed_verified_advances(verified: &RestoreRecord) -> Vec<RestoreRecord> {
    let verified_at = verified.data_verified_at.unwrap();
    let mut missing_completion = verified.clone();
    missing_completion.status = RestoreStatus::Succeeded;
    let mut early_completion = verified.clone();
    early_completion.status = RestoreStatus::Succeeded;
    early_completion.completed_at = Some(verified_at - Duration::nanoseconds(1));
    let mut imprecise_completion = verified.clone();
    imprecise_completion.status = RestoreStatus::Succeeded;
    imprecise_completion.completed_at = Some(verified_at + Duration::nanoseconds(1));
    let mut success_with_failure = verified.clone();
    success_with_failure.status = RestoreStatus::Succeeded;
    success_with_failure.completed_at = Some(verified_at);
    success_with_failure.failure = Some("成功状态不能带失败".into());
    let mut empty_failure = verified.clone();
    empty_failure.status = RestoreStatus::Failed;
    empty_failure.completed_at = Some(verified_at);
    empty_failure.failure = Some(String::new());
    let mut blank_failure = verified.clone();
    blank_failure.status = RestoreStatus::Failed;
    blank_failure.completed_at = Some(verified_at);
    blank_failure.failure = Some("   ".into());
    vec![
        missing_completion,
        early_completion,
        imprecise_completion,
        success_with_failure,
        empty_failure,
        blank_failure,
    ]
}

#[test]
fn backup_and_restore_inputs_reject_sub_microsecond_timestamps() {
    let current = chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let valid = manifest();
    let required = valid.resource_keys();
    let mut precise = valid.clone();
    precise.captured_at += Duration::microseconds(123);
    assert!(validate_backup_manifest(&precise, &required, current).is_ok());

    let mut invalid_times = Vec::new();
    let mut invalid = valid.clone();
    invalid.quiesced_at += Duration::nanoseconds(1);
    invalid_times.push(invalid);
    let mut invalid = valid.clone();
    invalid.captured_at += Duration::nanoseconds(1);
    invalid_times.push(invalid);
    let mut invalid = valid.clone();
    invalid.completed_at += Duration::nanoseconds(1);
    invalid_times.push(invalid);
    let mut invalid = valid.clone();
    invalid.retention_until += Duration::nanoseconds(1);
    invalid_times.push(invalid);
    for invalid in invalid_times {
        assert!(matches!(
            validate_backup_manifest(&invalid, &required, current),
            Err(AppError::Validation(_))
        ));
    }

    let backup = BackupRecord {
        manifest: valid,
        manifest_hash: String::new(),
        valid: true,
        checked_at: current,
        failure: None,
    };
    let mut precise = plan();
    precise.fault_at -= Duration::microseconds(123);
    assert!(validate_restore_plan(&backup, &precise, current).is_ok());
    let mut restore = plan();
    restore.fault_at += Duration::nanoseconds(1);
    assert!(matches!(
        validate_restore_plan(&backup, &restore, current),
        Err(AppError::Validation(_))
    ));
}

#[test]
fn backup_records_bind_hash_status_and_persisted_time() {
    let manifest = manifest();
    let mut record = BackupRecord {
        manifest_hash: backup_content_hash(&manifest).unwrap(),
        checked_at: manifest.completed_at,
        manifest,
        valid: true,
        failure: None,
    };
    assert!(validate_backup_record(&record).is_ok());

    let mut invalid = Vec::new();
    let mut candidate = record.clone();
    candidate.manifest_hash = "f".repeat(64);
    invalid.push(candidate);
    let mut candidate = record.clone();
    candidate.checked_at += Duration::nanoseconds(1);
    invalid.push(candidate);
    let mut candidate = record.clone();
    candidate.checked_at = candidate.manifest.completed_at - Duration::microseconds(1);
    invalid.push(candidate);
    let mut candidate = record.clone();
    candidate.checked_at = candidate.manifest.retention_until;
    invalid.push(candidate);
    let mut candidate = record.clone();
    candidate.failure = Some("不应存在".into());
    invalid.push(candidate);
    for detail in [None, Some("   ".into()), Some("错".repeat(1001))] {
        let mut candidate = record.clone();
        candidate.valid = false;
        candidate.failure = detail;
        invalid.push(candidate);
    }
    for candidate in invalid {
        assert!(matches!(
            validate_backup_record(&candidate),
            Err(AppError::Validation(_))
        ));
    }

    record.valid = false;
    record.failure = Some("校验失败".into());
    assert!(validate_backup_record(&record).is_ok());
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
    for case in 0..16 {
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
            7 => {
                proof.runner_sha.clear();
            }
            8 => {
                proof.tests_receipt_sha256.clear();
            }
            9 => {
                proof.backup_source_sha = "f".repeat(40);
            }
            10 => {
                proof.backend_execution_sha.clear();
            }
            11 => {
                proof.backend_execution_sha = "f".repeat(40);
            }
            12 => {
                proof.verifier_sha.clear();
            }
            13 => {
                proof.target_plan_sha256.clear();
            }
            14 => {
                proof.frontend_url.clear();
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
    assert!(matches!(
        service.register(changed, &manifest.resource_keys()).await,
        Err(AppError::Conflict(_))
    ));
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

#[test]
fn backup_restore_and_scope_identifiers_follow_the_formal_contract() {
    let original = manifest();
    let now = original.completed_at + Duration::minutes(5);
    let required = original.resource_keys();

    for id in ["a".to_owned(), format!("a{}", "_".repeat(63))] {
        let mut candidate = original.clone();
        candidate.id = id;
        validate_backup_manifest(&candidate, &required, now).unwrap();
    }
    for id in ["", "_backup", "-backup", "Backup", "backup.one"]
        .into_iter()
        .map(str::to_owned)
        .chain(["a".repeat(65)])
    {
        let mut candidate = original.clone();
        candidate.id = id;
        assert!(validate_backup_manifest(&candidate, &required, now).is_err());
    }

    for scope in ["a1".to_owned(), format!("a{}z", "_".repeat(46))] {
        let mut candidate = original.clone();
        candidate.scope_id = scope.clone();
        for objects in &mut candidate.objects {
            objects.prefix = format!("{scope}/");
        }
        validate_backup_manifest(&candidate, &required, now).unwrap();
    }
    for scope in [
        "",
        "a",
        "_scope",
        "-scope",
        "scope_",
        "scope-",
        "Scope",
        "scope.one",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([format!("a{}z", "_".repeat(47))])
    {
        let mut candidate = original.clone();
        candidate.scope_id = scope.clone();
        for objects in &mut candidate.objects {
            objects.prefix = format!("{scope}/");
        }
        assert!(validate_backup_manifest(&candidate, &required, now).is_err());
    }

    let backup = BackupRecord {
        manifest: original,
        manifest_hash: "e".repeat(64),
        valid: true,
        checked_at: now,
        failure: None,
    };
    for id in ["a".to_owned(), format!("a{}", "-".repeat(63))] {
        let mut candidate = plan();
        candidate.id = id;
        validate_restore_plan(&backup, &candidate, now).unwrap();
    }
    for id in ["", "_restore", "-restore", "Restore", "restore.one"]
        .into_iter()
        .map(str::to_owned)
        .chain(["a".repeat(65)])
    {
        let mut candidate = plan();
        candidate.id = id;
        assert!(validate_restore_plan(&backup, &candidate, now).is_err());
    }
    let mut invalid_backup = backup;
    invalid_backup.manifest.id = "_backup".into();
    let mut candidate = plan();
    candidate.backup_id = invalid_backup.manifest.id.clone();
    assert!(validate_restore_plan(&invalid_backup, &candidate, now).is_err());
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
