use super::*;

pub(super) async fn check_restore_rejects_corrupt_point(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    backup: &BackupRecord,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let running = restore_record(
        backup,
        "restore-corrupt-point",
        now,
        now - Duration::seconds(30),
    );
    persist_create(repository, &running).await?;
    let mut verified = running.clone();
    verified.status = RestoreStatus::DataVerified;
    verified.data_verified_at = Some(now - Duration::seconds(5));
    let verified = persist_advance(repository, &running, &verified).await?;
    let mut succeeded = verified.clone();
    succeeded.status = RestoreStatus::Succeeded;
    succeeded.completed_at = Some(now);
    execute(
        database,
        "UPDATE sys_tenant_data_backup_point SET validation_status = 'invalid', \
         validation_detail = NULL \
         WHERE provider_ref = 'backup-set:backup-integration:shared-control'",
    )
    .await?;
    expect_rejected_advance(
        repository,
        &verified,
        &succeeded,
        "备份点校验状态不一致",
        "备份点校验状态损坏后恢复状态没有回滚",
    )
    .await?;
    execute(
        database,
        "UPDATE sys_tenant_data_backup_point SET validation_status = 'valid', \
         validation_detail = 'tampered validation detail' \
         WHERE provider_ref = 'backup-set:backup-integration:shared-control'",
    )
    .await?;
    expect_rejected_advance(
        repository,
        &verified,
        &succeeded,
        "备份点校验状态不一致",
        "备份点校验详情损坏后恢复状态没有回滚",
    )
    .await?;
    execute(
        database,
        "UPDATE sys_tenant_data_backup_point SET validation_detail = NULL, \
         target_key = 'tampered-target' \
         WHERE provider_ref = 'backup-set:backup-integration:shared-control'",
    )
    .await?;
    expect_rejected_advance(
        repository,
        &verified,
        &succeeded,
        "不可变投影",
        "备份点投影损坏后恢复状态没有回滚",
    )
    .await?;
    execute(
        database,
        "UPDATE sys_tenant_data_backup_point SET target_key = 'shared-control' \
         WHERE provider_ref = 'backup-set:backup-integration:shared-control'",
    )
    .await?;
    close_verified_restore(repository, &verified, now, "备份点损坏已验证").await
}

pub(super) async fn check_restore_rejects_invalid_backup(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    backup: &BackupRecord,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let running = restore_record(
        backup,
        "restore-invalid-backup",
        now,
        now - Duration::seconds(20),
    );
    persist_create(repository, &running).await?;
    let mut verified = running.clone();
    verified.status = RestoreStatus::DataVerified;
    verified.data_verified_at = Some(now - Duration::seconds(5));
    let verified = persist_advance(repository, &running, &verified).await?;
    let mut invalid = backup.clone();
    invalid.checked_at = now + Duration::microseconds(1);
    invalid.valid = false;
    invalid.failure = Some("完成前复验失败".into());
    save(repository, &invalid).await?;
    execute(
        database,
        "UPDATE sys_tenant_data_backup_point SET validation_status = 'valid', \
         validation_detail = NULL \
         WHERE provider_ref = 'backup-set:backup-integration:shared-control'",
    )
    .await?;
    let mut succeeded = verified.clone();
    succeeded.status = RestoreStatus::Succeeded;
    succeeded.completed_at = Some(now);
    expect_rejected_advance(
        repository,
        &verified,
        &succeeded,
        "备份已经无效或过期",
        "备份失效后恢复状态没有回滚",
    )
    .await?;
    let mut repaired = invalid;
    repaired.checked_at = now + Duration::microseconds(2);
    repaired.valid = true;
    repaired.failure = None;
    save(repository, &repaired).await?;
    close_verified_restore(repository, &verified, now, "备份失效防护已验证").await
}

async fn expect_rejected_advance(
    repository: &dyn BackupRepository,
    verified: &RestoreRecord,
    succeeded: &RestoreRecord,
    expected_error: &str,
    rollback_error: &str,
) -> Result<(), String> {
    let error = match persist_advance(repository, verified, succeeded).await {
        Ok(_) => return Err("损坏的备份状态错误地完成了恢复演练".into()),
        Err(error) => error,
    };
    check(error.contains(expected_error), "恢复演练防护返回了无关错误")?;
    check(
        repository
            .restore(&verified.plan.id)
            .await
            .map_err(|error| error.to_string())?
            == Some(verified.clone()),
        rollback_error,
    )
}

async fn close_verified_restore(
    repository: &dyn BackupRepository,
    verified: &RestoreRecord,
    completed_at: DateTime<Utc>,
    failure: &str,
) -> Result<(), String> {
    let mut failed = verified.clone();
    failed.status = RestoreStatus::Failed;
    failed.completed_at = Some(completed_at);
    failed.failure = Some(failure.into());
    persist_advance(repository, verified, &failed)
        .await
        .map(|_| ())
}
