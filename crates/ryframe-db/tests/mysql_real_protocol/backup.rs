use super::{ControlDatabaseCluster, application_ports, execute, require_count, run_mysql_test};
use chrono::{DateTime, Duration, Utc};
use ryframe_application::ports::backup::*;
use ryframe_kernel::AppResult;
use sea_orm::{IsolationLevel, TransactionTrait};
use std::sync::atomic::{AtomicI64, Ordering};

#[path = "backup/restore_safety.rs"]
mod restore_safety;

fn next_id() -> AppResult<i64> {
    static NEXT: AtomicI64 = AtomicI64::new(900_000);
    Ok(NEXT.fetch_add(1, Ordering::Relaxed))
}

fn check(condition: bool, message: &str) -> Result<(), String> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_transactions_health_and_restore_states_use_real_mysql() {
    run_mysql_test("backup", |database| async move {
        ryframe_db::install_id_generator(next_id).map_err(|error| error.to_string())?;
        ryframe_db::migration::up(&database).await.map_err(|error| error.to_string())?;
        let repository = application_ports::backup::port(ControlDatabaseCluster::single(database.clone()));
        let now = repository.database_now().await.map_err(|error| error.to_string())?;
        let now = DateTime::from_timestamp(now.timestamp(), 0).ok_or("数据库时间无效")?;
        let record = record(now);
        let transaction = repository.begin().await.map_err(|error| error.to_string())?;
        transaction.save_backup(&record).await.map_err(|error| error.to_string())?;
        drop(transaction);
        check(repository.backup(&record.manifest.id).await.map_err(|error| error.to_string())?.is_none(), "未提交的登记不得可见")?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_tenant_data_backup_point \
             WHERE provider_ref = 'backup-set:backup-integration:shared-control'",
            0,
        )
        .await?;
        save(repository.as_ref(), &record).await?;
        let stored = repository.backup(&record.manifest.id).await.map_err(|error| error.to_string())?.ok_or("备份记录丢失")?;
        check(stored.manifest == record.manifest, "备份清单往返发生变化")?;
        require_count(&database, "SELECT COUNT(*) AS value FROM sys_tenant_data_backup_point", 1).await?;
        check_health(repository.as_ref(), &database, &record, now).await?;
        check_restores(repository.as_ref(), &database, &record, now).await?;
        check_registration_concurrency(repository.as_ref(), &database, now).await?;
        require_count(&database, "SELECT COUNT(*) AS value FROM sys_tenant_data_backup_point WHERE last_restore_drill_at IS NOT NULL", 1).await?;
        check_projection_corruption(repository.as_ref(), &database, now).await
    }).await;
}

async fn save(repository: &dyn BackupRepository, record: &BackupRecord) -> Result<(), String> {
    let transaction = repository
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .save_backup(record)
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())
}

async fn check_health(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    record: &BackupRecord,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let required = repository
        .required_resources()
        .await
        .map_err(|error| error.to_string())?;
    check(
        required.len() == 6,
        "初始化控制库必须登记控制目标及五个对象桶",
    )?;
    let scope = &record.manifest.scope_id;
    let health = repository
        .health(scope, &required, now)
        .await
        .map_err(|error| error.to_string())?;
    check(
        health.missing_resources == 0 && health.oldest_capture == Some(record.manifest.captured_at),
        "登记时间不能替代实际采集时间",
    )?;
    let unrelated = repository
        .health("another-scope", &required, now)
        .await
        .map_err(|error| error.to_string())?;
    check(
        unrelated.missing_resources == 6 && unrelated.oldest_capture.is_none(),
        "跨 scope 备份泄漏",
    )?;
    let mut invalid = record.clone();
    invalid.manifest.id = "backup-invalid".into();
    invalid.manifest.captured_at = now - Duration::hours(1);
    invalid.manifest_hash =
        backup_content_hash(&invalid.manifest).map_err(|error| error.to_string())?;
    invalid.valid = false;
    invalid.failure = Some("校验和错误".into());
    save(repository, &invalid).await?;
    require_count(
        database,
        "SELECT COUNT(*) AS value FROM sys_tenant_data_backup_point \
         WHERE provider_ref = 'backup-set:backup-invalid:shared-control' \
         AND validation_status = 'invalid' AND validation_detail = '校验和错误'",
        1,
    )
    .await?;
    let health = repository
        .health(scope, &required, now)
        .await
        .map_err(|error| error.to_string())?;
    check(
        health.invalid_resources == 6 && health.oldest_capture == Some(record.manifest.captured_at),
        "较新的失败备份不能掩盖有效备份年龄",
    )?;
    let expired = repository
        .health(scope, &required, now + Duration::days(10))
        .await
        .map_err(|error| error.to_string())?;
    check(
        expired.expired_resources == 6 && expired.oldest_capture.is_none(),
        "过期备份不得作为可用恢复点",
    )
}

async fn check_restores(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    backup: &BackupRecord,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let required = backup.manifest.resource_keys();
    check_overdue_restore(repository, backup, &required, now).await?;
    check_restore_create_wait(repository, backup, now).await?;
    check_restore_projection_rollback(repository, database, backup, now).await?;
    restore_safety::check_restore_rejects_corrupt_point(repository, database, backup, now).await?;
    restore_safety::check_restore_rejects_invalid_backup(repository, database, backup, now).await?;
    check_restore_success_and_cas(repository, backup, now).await?;
    check_restore_drill_monotonic(repository, database, backup, now).await?;
    check_failed_restore_and_health(repository, backup, &required, now).await
}

async fn check_overdue_restore(
    repository: &dyn BackupRepository,
    backup: &BackupRecord,
    required: &[String],
    now: DateTime<Utc>,
) -> Result<(), String> {
    let overdue = restore_record(
        backup,
        "restore-overdue",
        now,
        now - Duration::seconds(4000),
    );
    persist_create(repository, &overdue).await?;
    let health = repository
        .health(&backup.manifest.scope_id, required, now)
        .await
        .map_err(|error| error.to_string())?;
    check(
        health.restore_running == 1 && health.restore_overdue == 1,
        "超时演练计数错误",
    )
}

async fn check_restore_create_wait(
    repository: &dyn BackupRepository,
    backup: &BackupRecord,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let concurrent = restore_record(
        backup,
        "restore-create-race",
        now,
        now - Duration::seconds(120),
    );
    let mut concurrent_retry = concurrent.clone();
    concurrent_retry.started_at += Duration::seconds(5);
    let first_transaction = repository
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    let first = first_transaction
        .create_restore(&concurrent)
        .await
        .map_err(|error| error.to_string())?;
    let second_transaction = repository
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    let mut second_run = Box::pin(second_transaction.create_restore(&concurrent_retry));
    check(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut second_run)
            .await
            .is_err(),
        "未提交赢家存在时，并发恢复创建没有等待赢家事务",
    )?;
    first_transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    let second = tokio::time::timeout(std::time::Duration::from_secs(10), &mut second_run)
        .await
        .map_err(|_| "赢家提交后并发恢复创建没有继续")?
        .map_err(|error| error.to_string())?;
    drop(second_run);
    second_transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    check(
        first == concurrent && second == first,
        "并发首次创建未返回同一权威记录",
    )?;
    let mut concurrent_failed = first.clone();
    concurrent_failed.status = RestoreStatus::Failed;
    concurrent_failed.completed_at = Some(now - Duration::seconds(1));
    concurrent_failed.failure = Some("并发创建测试结束".into());
    persist_advance(repository, &first, &concurrent_failed)
        .await
        .map(|_| ())
}

async fn check_restore_projection_rollback(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    backup: &BackupRecord,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let rollback_running = restore_record(
        backup,
        "restore-projection-rollback",
        now,
        now - Duration::seconds(45),
    );
    persist_create(repository, &rollback_running).await?;
    let mut rollback_verified = rollback_running.clone();
    rollback_verified.status = RestoreStatus::DataVerified;
    rollback_verified.data_verified_at = Some(now - Duration::seconds(5));
    let rollback_verified =
        persist_advance(repository, &rollback_running, &rollback_verified).await?;
    execute(
        database,
        "DELETE FROM sys_tenant_data_backup_point \
         WHERE provider_ref = 'backup-set:backup-integration:shared-control'",
    )
    .await?;
    let mut rollback_succeeded = rollback_verified.clone();
    rollback_succeeded.status = RestoreStatus::Succeeded;
    rollback_succeeded.completed_at = Some(now);
    check(
        persist_advance(repository, &rollback_verified, &rollback_succeeded)
            .await
            .is_err(),
        "终态投影缺失时恢复演练不得提交成功",
    )?;
    check(
        repository
            .restore(&rollback_verified.plan.id)
            .await
            .map_err(|error| error.to_string())?
            == Some(rollback_verified.clone()),
        "终态投影失败后恢复状态没有随事务回滚",
    )?;
    save(repository, backup).await?;
    require_count(
        database,
        "SELECT COUNT(*) AS value FROM sys_tenant_data_backup_point \
         WHERE provider_ref = 'backup-set:backup-integration:shared-control'",
        1,
    )
    .await?;
    let mut rollback_failed = rollback_verified.clone();
    rollback_failed.status = RestoreStatus::Failed;
    rollback_failed.completed_at = Some(now);
    rollback_failed.failure = Some("投影故障注入已验证".into());
    persist_advance(repository, &rollback_verified, &rollback_failed)
        .await
        .map(|_| ())
}

async fn check_restore_success_and_cas(
    repository: &dyn BackupRepository,
    backup: &BackupRecord,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let running = restore_record(backup, "restore-success", now, now - Duration::seconds(60));
    let created = persist_create(repository, &running).await?;
    check(created == running, "首次创建未返回权威记录")?;
    let mut retry = running.clone();
    retry.started_at += Duration::seconds(5);
    retry.recovered_at += Duration::seconds(5);
    let retried = persist_create(repository, &retry).await?;
    check(retried == running, "同计划重试未返回首次权威记录")?;

    let mut conflicting = running.clone();
    conflicting.plan.scope_id = "another-isolated-target".into();
    conflicting.plan.object_prefix = "another-isolated-target/".into();
    conflicting.plan_hash =
        backup_content_hash(&conflicting.plan).map_err(|error| error.to_string())?;
    let error = persist_create(repository, &conflicting).await.unwrap_err();
    check(error.contains("数据冲突"), "同 ID 不同计划未返回冲突")?;
    let mut conflicting_hash = running.clone();
    conflicting_hash.plan_hash = "f".repeat(64);
    let error = persist_create(repository, &conflicting_hash)
        .await
        .unwrap_err();
    check(error.contains("数据冲突"), "同 ID 不同摘要未返回冲突")?;

    let mut stale = running.clone();
    stale.started_at -= Duration::seconds(1);
    let mut stale_next = stale.clone();
    stale_next.status = RestoreStatus::DataVerified;
    stale_next.data_verified_at = Some(now - Duration::seconds(30));
    let error = persist_advance(repository, &stale, &stale_next)
        .await
        .unwrap_err();
    check(error.contains("数据冲突"), "完整旧记录不匹配时 CAS 未失败")?;

    let mut verified = running.clone();
    verified.status = RestoreStatus::DataVerified;
    verified.data_verified_at = Some(now - Duration::seconds(30));
    let verified = persist_advance(repository, &running, &verified).await?;
    let mut succeeded = verified.clone();
    succeeded.status = RestoreStatus::Succeeded;
    succeeded.completed_at = Some(now);
    let first_transaction = repository
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    let first = first_transaction
        .advance_restore(&verified, &succeeded)
        .await
        .map_err(|error| error.to_string())?;
    let second_transaction = repository
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    let mut second_run = Box::pin(second_transaction.advance_restore(&verified, &succeeded));
    check(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut second_run)
            .await
            .is_err(),
        "未提交赢家存在时，并发终态 CAS 没有等待行锁",
    )?;
    first_transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    let second = tokio::time::timeout(std::time::Duration::from_secs(10), &mut second_run)
        .await
        .map_err(|_| "赢家提交后并发终态 CAS 没有继续")?;
    drop(second_run);
    drop(second_transaction);
    check(first == succeeded, "并发终态赢家没有返回权威记录")?;
    let failed = second.expect_err("第二个终态 CAS 必须失败").to_string();
    check(failed.contains("数据冲突"), "并发终态失败者不是 CAS 冲突")
}

async fn check_failed_restore_and_health(
    repository: &dyn BackupRepository,
    backup: &BackupRecord,
    required: &[String],
    now: DateTime<Utc>,
) -> Result<(), String> {
    let running_failure =
        restore_record(backup, "restore-failure", now, now - Duration::seconds(90));
    persist_create(repository, &running_failure).await?;
    let mut failure = running_failure.clone();
    failure.status = RestoreStatus::Failed;
    failure.completed_at = Some(now + Duration::seconds(1));
    failure.failure = Some("业务验证失败".into());
    persist_advance(repository, &running_failure, &failure).await?;

    let health = repository
        .health(
            &backup.manifest.scope_id,
            required,
            now + Duration::seconds(1),
        )
        .await
        .map_err(|error| error.to_string())?;
    check(
        health.restore_running == 1
            && !health.last_restore_succeeded
            && health.restore_duration_seconds == Some(91)
            && health.recovery_point_age_seconds == Some(24 * 3600),
        "来源 scope 的最新恢复结果或时间统计错误",
    )?;
    let target_health = repository
        .health("isolated-restored", required, now + Duration::seconds(1))
        .await
        .map_err(|error| error.to_string())?;
    check(
        target_health.last_restore_completed.is_none() && target_health.restore_running == 0,
        "恢复健康状态错误地按目标 scope 聚合",
    )
}

async fn check_restore_drill_monotonic(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    backup: &BackupRecord,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let running = restore_record(
        backup,
        "restore-older-completion",
        now,
        now - Duration::seconds(120),
    );
    persist_create(repository, &running).await?;
    let mut verified = running.clone();
    verified.status = RestoreStatus::DataVerified;
    verified.data_verified_at = Some(now - Duration::seconds(60));
    let verified = persist_advance(repository, &running, &verified).await?;
    let mut succeeded = verified.clone();
    succeeded.status = RestoreStatus::Succeeded;
    succeeded.completed_at = Some(now - Duration::seconds(1));
    persist_advance(repository, &verified, &succeeded).await?;
    require_count(
        database,
        &format!(
            "SELECT COUNT(*) AS value FROM sys_tenant_data_backup_point \
             WHERE provider_ref = 'backup-set:backup-integration:shared-control' \
             AND last_restore_drill_at = '{}'",
            now.format("%Y-%m-%d %H:%M:%S%.6f")
        ),
        1,
    )
    .await
}

async fn persist_create(
    repository: &dyn BackupRepository,
    record: &RestoreRecord,
) -> Result<RestoreRecord, String> {
    let transaction = repository
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    let authoritative = transaction
        .create_restore(record)
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(authoritative)
}

async fn persist_advance(
    repository: &dyn BackupRepository,
    expected: &RestoreRecord,
    next: &RestoreRecord,
) -> Result<RestoreRecord, String> {
    let transaction = repository
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    let authoritative = transaction
        .advance_restore(expected, next)
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    Ok(authoritative)
}

async fn check_registration_concurrency(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<(), String> {
    for (name, isolation) in [
        ("read-committed", IsolationLevel::ReadCommitted),
        ("repeatable-read", IsolationLevel::RepeatableRead),
    ] {
        let record = record_with_id(now, &format!("backup-same-{name}"));
        check_same_manifest_wait(database, &record, isolation, name).await?;
    }
    for (suffix, isolation) in [
        ("rc", IsolationLevel::ReadCommitted),
        ("rr", IsolationLevel::RepeatableRead),
    ] {
        let winner = record_with_id(now, &format!("backup-conflicting-{suffix}"));
        let mut loser = winner.clone();
        loser.manifest.databases[0].key = "alternate-control".into();
        loser.manifest_hash =
            backup_content_hash(&loser.manifest).map_err(|error| error.to_string())?;
        check_conflicting_manifest_wait(repository, database, &winner, &loser, isolation).await?;
    }
    check_revalidation_order(repository, database, now).await
}

async fn check_same_manifest_wait(
    database: &sea_orm::DatabaseConnection,
    record: &BackupRecord,
    isolation: IsolationLevel,
    name: &str,
) -> Result<(), String> {
    let winner_transaction = database
        .begin_with_config(Some(isolation), None)
        .await
        .map_err(|error| error.to_string())?;
    let winner = ryframe_db::repositories::backup_repo::save_backup(&winner_transaction, record)
        .await
        .map_err(|error| error.to_string())?;
    let loser_transaction = database
        .begin_with_config(Some(isolation), None)
        .await
        .map_err(|error| error.to_string())?;
    let mut loser_run = Box::pin(ryframe_db::repositories::backup_repo::save_backup(
        &loser_transaction,
        record,
    ));
    check(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut loser_run)
            .await
            .is_err(),
        &format!("{name} 未提交赢家存在时，重复登记没有等待唯一键"),
    )?;
    winner_transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    let loser = tokio::time::timeout(std::time::Duration::from_secs(10), &mut loser_run)
        .await
        .map_err(|_| format!("{name} 赢家提交后重复登记没有继续"))?
        .map_err(|error| error.to_string())?;
    drop(loser_run);
    loser_transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    check(
        winner == *record && loser == *record,
        "同清单并发登记未幂等",
    )?;
    require_count(
        database,
        &format!(
            "SELECT COUNT(*) AS value FROM sys_backup_resource WHERE backup_id = '{}'",
            record.manifest.id
        ),
        record.manifest.resource_keys().len() as i64,
    )
    .await
}

async fn check_conflicting_manifest_wait(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    winner: &BackupRecord,
    loser: &BackupRecord,
    isolation: IsolationLevel,
) -> Result<(), String> {
    let winner_transaction = database
        .begin_with_config(Some(isolation), None)
        .await
        .map_err(|error| error.to_string())?;
    ryframe_db::repositories::backup_repo::save_backup(&winner_transaction, winner)
        .await
        .map_err(|error| error.to_string())?;
    let loser_transaction = database
        .begin_with_config(Some(isolation), None)
        .await
        .map_err(|error| error.to_string())?;
    let mut loser_run = Box::pin(ryframe_db::repositories::backup_repo::save_backup(
        &loser_transaction,
        loser,
    ));
    check(
        tokio::time::timeout(std::time::Duration::from_millis(200), &mut loser_run)
            .await
            .is_err(),
        "未提交赢家存在时，异清单登记没有等待唯一键",
    )?;
    winner_transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    let error = tokio::time::timeout(std::time::Duration::from_secs(10), &mut loser_run)
        .await
        .map_err(|_| "赢家提交后异清单登记没有继续")?
        .expect_err("异清单重复登记必须失败")
        .to_string();
    drop(loser_run);
    loser_transaction
        .rollback()
        .await
        .map_err(|rollback| rollback.to_string())?;
    check(error.contains("数据冲突"), "异清单失败者不是业务冲突")?;
    check(
        repository
            .backup(&winner.manifest.id)
            .await
            .map_err(|error| error.to_string())?
            == Some(winner.clone()),
        "异清单并发登记产生混合父记录",
    )?;
    assert_exact_backup_projection(database, winner, [winner, loser]).await
}

async fn persist_direct(
    database: &sea_orm::DatabaseConnection,
    record: &BackupRecord,
    isolation: IsolationLevel,
) -> Result<BackupRecord, String> {
    let transaction = database
        .begin_with_config(Some(isolation), None)
        .await
        .map_err(|error| error.to_string())?;
    match ryframe_db::repositories::backup_repo::save_backup(&transaction, record).await {
        Ok(authoritative) => {
            transaction
                .commit()
                .await
                .map_err(|error| error.to_string())?;
            Ok(authoritative)
        }
        Err(error) => {
            transaction
                .rollback()
                .await
                .map_err(|rollback| rollback.to_string())?;
            Err(error.to_string())
        }
    }
}

async fn assert_exact_backup_projection(
    database: &sea_orm::DatabaseConnection,
    winner: &BackupRecord,
    candidates: [&BackupRecord; 2],
) -> Result<(), String> {
    let winner_keys = winner.manifest.resource_keys();
    require_count(
        database,
        &format!(
            "SELECT COUNT(*) AS value FROM sys_backup_resource WHERE backup_id = '{}'",
            winner.manifest.id
        ),
        winner_keys.len() as i64,
    )
    .await?;
    for candidate in candidates {
        let target = &candidate.manifest.databases[0].key;
        let expected = i64::from(winner_keys.contains(&format!("db:{target}")));
        require_count(
            database,
            &format!(
                "SELECT COUNT(*) AS value FROM sys_backup_resource \
                 WHERE backup_id = '{}' AND resource_key = 'db:{}'",
                winner.manifest.id, target
            ),
            expected,
        )
        .await?;
        require_count(
            database,
            &format!(
                "SELECT COUNT(*) AS value FROM sys_tenant_data_backup_point \
                 WHERE provider_ref = 'backup-set:{}:{}'",
                winner.manifest.id, target
            ),
            expected,
        )
        .await?;
    }
    Ok(())
}

async fn check_revalidation_order(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let mut invalid = record_with_id(now, "backup-revalidation-order");
    invalid.checked_at = now - Duration::seconds(2);
    invalid.valid = false;
    invalid.failure = Some("旧校验失败".into());
    persist_direct(database, &invalid, IsolationLevel::ReadCommitted).await?;
    let mut valid = invalid.clone();
    valid.checked_at = now - Duration::seconds(1);
    valid.valid = true;
    valid.failure = None;
    let valid = persist_direct(database, &valid, IsolationLevel::ReadCommitted).await?;
    let stale = persist_direct(database, &invalid, IsolationLevel::ReadCommitted).await?;
    check(stale == valid, "旧校验结果覆盖了较新权威记录")?;
    check(
        repository
            .backup(&valid.manifest.id)
            .await
            .map_err(|error| error.to_string())?
            == Some(valid.clone()),
        "较新备份校验记录没有保持权威",
    )?;
    require_count(
        database,
        "SELECT COUNT(*) AS value FROM sys_tenant_data_backup_point \
         WHERE provider_ref = 'backup-set:backup-revalidation-order:shared-control' \
         AND validation_status = 'valid' AND validation_detail IS NULL",
        1,
    )
    .await
}

fn restore_record(
    backup: &BackupRecord,
    id: &str,
    fault_at: DateTime<Utc>,
    started_at: DateTime<Utc>,
) -> RestoreRecord {
    let plan = RestorePlan {
        id: id.into(),
        backup_id: backup.manifest.id.clone(),
        scope_id: "isolated-restored".into(),
        fault_at,
        databases: vec![RestoreDatabase {
            source_key: "shared-control".into(),
            target_key: "isolated-control".into(),
            server_uuid: "restore-server".into(),
            database: "restore_database".into(),
        }],
        object_endpoint: "http://127.0.0.1:19000".into(),
        object_prefix: "isolated-restored/".into(),
        api_ready_url: "http://127.0.0.1:18080/readyz".into(),
        worker_ready_url: "http://127.0.0.1:19091/readyz".into(),
        frontend_sha: "b".repeat(40),
    };
    RestoreRecord {
        plan_hash: backup_content_hash(&plan).expect("测试恢复计划必须可计算摘要"),
        plan,
        status: RestoreStatus::Running,
        started_at,
        data_verified_at: None,
        completed_at: None,
        recovered_at: backup.manifest.captured_at,
        failure: None,
    }
}

async fn check_projection_corruption(
    repository: &dyn BackupRepository,
    database: &sea_orm::DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let mut health_candidate = record_with_id(now, "backup-health-corrupt");
    health_candidate.manifest.scope_id = "corrupt-scope".into();
    for objects in &mut health_candidate.manifest.objects {
        objects.prefix = "corrupt-scope/".into();
    }
    health_candidate.manifest_hash =
        backup_content_hash(&health_candidate.manifest).map_err(|error| error.to_string())?;
    save(repository, &health_candidate).await?;
    execute(
        database,
        "UPDATE sys_backup_set SET manifest_hash = REPEAT('f', 64) \
         WHERE id = 'backup-health-corrupt'",
    )
    .await?;
    check(
        repository
            .health(
                &health_candidate.manifest.scope_id,
                &health_candidate.manifest.resource_keys(),
                now,
            )
            .await
            .is_err(),
        "健康状态接受了摘要损坏的有效备份候选",
    )?;
    execute(
        database,
        "DELETE FROM sys_backup_resource WHERE backup_id = 'backup-invalid' LIMIT 1",
    )
    .await?;
    check(
        repository.backup("backup-invalid").await.is_err(),
        "备份读取未核对资源关系投影",
    )?;
    execute(
        database,
        "UPDATE sys_backup_set SET manifest_hash = REPEAT('f', 64) \
         WHERE id = 'backup-integration'",
    )
    .await?;
    check(
        repository.backup("backup-integration").await.is_err(),
        "备份读取未重算清单摘要",
    )?;
    execute(
        database,
        "UPDATE sys_restore_run SET payload = JSON_SET(payload, '$.plan_hash', REPEAT('f', 64)) \
         WHERE id = 'restore-failure'",
    )
    .await?;
    check(
        repository.restore("restore-failure").await.is_err(),
        "恢复读取未重算计划摘要",
    )?;
    execute(
        database,
        "UPDATE sys_restore_run SET scope_id = 'tampered-target' \
         WHERE id = 'restore-success'",
    )
    .await?;
    check(
        repository.restore("restore-success").await.is_err(),
        "恢复读取未核对关系投影",
    )
}

fn record(now: DateTime<Utc>) -> BackupRecord {
    let captured = now - Duration::hours(24);
    let manifest = BackupManifest {
        id: "backup-integration".into(),
        scope_id: "integration-scope".into(),
        source_sha: "a".repeat(40),
        quiesced_at: captured,
        captured_at: captured,
        completed_at: captured + Duration::minutes(1),
        retention_until: now + Duration::days(7),
        control_schema_fingerprint: "control".into(),
        tenant_schema_fingerprint: "tenant".into(),
        databases: vec![DatabaseBackup {
            key: "shared-control".into(),
            kind: BackupDatabaseKind::Combined,
            server_uuid: "test-server".into(),
            database: "test-database".into(),
            shared: true,
            placements: vec![],
            tables: vec![],
        }],
        objects: ["uploads", "avatar", "exports", "imports", "config-packages"]
            .into_iter()
            .map(|bucket| ObjectBackup {
                bucket: bucket.into(),
                prefix: "integration-scope/".into(),
                entries: vec![],
            })
            .collect(),
        artifacts: vec![],
    };
    BackupRecord {
        manifest_hash: backup_content_hash(&manifest).expect("测试备份清单必须可计算摘要"),
        manifest,
        valid: true,
        checked_at: now,
        failure: None,
    }
}

fn record_with_id(now: DateTime<Utc>, id: &str) -> BackupRecord {
    let mut record = record(now);
    record.manifest.id = id.into();
    record.manifest_hash = backup_content_hash(&record.manifest).expect("测试清单必须可计算摘要");
    record
}
