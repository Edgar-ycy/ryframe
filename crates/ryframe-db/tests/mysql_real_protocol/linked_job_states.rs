use ryframe_application::ports::jobs::{ExecutionTenantScope, FailJobCommand, JobFailureOutcome};
use ryframe_db::{ControlDatabaseCluster, application_ports, migration};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use time::{Duration, OffsetDateTime, PrimitiveDateTime, macros::offset};

use super::mysql::{execute, require_count, run_mysql_test};

#[cfg(feature = "migration")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outbox_delivery_keeps_utc_lease_and_atomic_queue_boundaries() {
    use ryframe_application::{
        EnqueueJob, TransactionAuditMode,
        ports::{jobs::OutboxFailureOutcome, system::MessageOutboxRecord},
    };
    use time::macros::datetime;

    run_mysql_test("outbox-utc-boundary", |database| async move {
        install_control_schema(&database).await?;
        super::install_test_database_id_generator()?;
        let now = datetime!(2026-11-26 01:00:00 +01:00);
        let lease = Duration::seconds(5);
        let retry_at = now + Duration::seconds(2);
        let scope = ExecutionTenantScope::tenant_and_platform("tenant-outbox");
        let cluster = ControlDatabaseCluster::single(database.clone());
        let messages = application_ports::system::message(cluster.clone());
        let transaction = messages.begin().await.map_err(|error| error.to_string())?;
        transaction
            .record_outbox(MessageOutboxRecord {
                    tenant_id: "tenant-outbox".into(),
                    event_type: "system.message.published".into(),
                    aggregate_id: "42".into(),
                    payload: serde_json::json!({"message_id":"42"}),
                    available_at: now,
                    traceparent: None,
                    tracestate: None,
                })
            .await
            .map_err(|error| error.to_string())?;
        transaction.commit(TransactionAuditMode::Skip).await.map_err(|error| error.to_string())?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_outbox_event WHERE dedupe_key = 'message:42' \
             AND available_at = '2026-11-26 00:00:00' AND created_at = '2026-11-26 00:00:00' \
             AND updated_at = '2026-11-26 00:00:00'",
            1,
        )
        .await?;
        let outbox = application_ports::jobs::outbox(cluster);
        assert!(outbox.claim_next("first", lease, now - Duration::SECOND, &scope).await.map_err(|error| error.to_string())?.is_none());
        let first = outbox.claim_next("first", lease, now, &scope).await
            .map_err(|error| error.to_string())?.expect("事件到期时应可以领取");
        let event_id = first.id;
        assert_eq!(first.attempts, 1);
        assert_eq!(first.dedupe_key.as_deref(), Some("message:42"));
        assert_eq!(
            outbox.fail(event_id, "first", retry_at, "retry", now + Duration::SECOND)
                .await.map_err(|error| error.to_string())?,
            OutboxFailureOutcome::Retried { available_at: retry_at }
        );
        assert!(outbox.claim_next("second", lease, retry_at - Duration::SECOND, &scope).await.map_err(|error| error.to_string())?.is_none());
        let second = outbox.claim_next("second", lease, retry_at, &scope).await
            .map_err(|error| error.to_string())?.expect("重试时间到达时应可领取");
        assert_eq!((second.id, second.attempts), (event_id, 2));

        let expires_at = retry_at + lease;
        let job = || EnqueueJob {
            tenant_id: Some("tenant-outbox".into()),
            schedule_id: None,
            scheduled_for: Some(expires_at),
            max_runtime_seconds: None,
            job_type: "system.message.dispatch".into(),
            payload: serde_json::json!({"message_id":"42"}),
            priority: 10,
            available_at: expires_at,
            max_attempts: 3,
            dedupe_key: Some("outbox-utc-42".into()),
            traceparent: None,
            tracestate: None,
        };
        let jobs = "SELECT COUNT(*) AS value FROM sys_background_job WHERE dedupe_key = 'outbox-utc-42'";
        assert!(!outbox.publish_background_job(event_id, "second", job(), expires_at)
            .await.map_err(|error| error.to_string())?);
        require_count(&database, jobs, 0).await?;
        outbox.recover_expired_leases(expires_at, &scope).await.map_err(|error| error.to_string())?;
        let reclaimed = outbox.claim_next("third", lease, expires_at, &scope).await
            .map_err(|error| error.to_string())?.expect("到期租约应归还领取");
        assert_eq!((reclaimed.id, reclaimed.attempts), (event_id, 3));
        assert!(outbox.publish_background_job(event_id, "third", job(), expires_at)
            .await.map_err(|error| error.to_string())?);
        require_count(&database, jobs, 1).await?;
        require_count(&database,
            "SELECT COUNT(*) AS value FROM sys_outbox_event WHERE dedupe_key = 'message:42' \
             AND status = 'published' AND attempts = 3 AND lease_owner IS NULL AND lease_until IS NULL \
             AND published_at = '2026-11-26 00:00:07' AND updated_at = '2026-11-26 00:00:07'",
            1,
        ).await
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_terminal_linked_jobs_do_not_requeue() {
    run_mysql_test("linked-terminal", |database| async move {
        install_control_schema(&database).await?;
        seed_expired_terminal_imports(&database).await?;

        let persistence =
            application_ports::jobs::queue(ControlDatabaseCluster::single(database.clone()));
        let now = OffsetDateTime::now_utc();
        let recovered = persistence
            .recover_expired_leases(now, &ExecutionTenantScope::all())
            .await
            .map_err(|error| error.to_string())?;
        if recovered.requeued != 0 || recovered.completed != 1 || recovered.dead != 1 {
            return Err(format!("关联终态回收计数错误: {recovered:?}"));
        }

        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job \
             WHERE id = 7101 AND status = 'succeeded' AND lease_owner IS NULL \
             AND lease_until IS NULL AND completed_at IS NOT NULL",
            1,
        )
        .await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job \
             WHERE id = 7102 AND status = 'dead' AND lease_owner IS NULL \
             AND lease_until IS NULL AND completed_at IS NOT NULL",
            1,
        )
        .await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job WHERE status = 'pending'",
            0,
        )
        .await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_user_import_job \
             WHERE (id = 7201 AND status = 'succeeded') \
                OR (id = 7202 AND status = 'failed')",
            2,
        )
        .await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job_attempt \
             WHERE job_id IN (7101, 7102) AND outcome = 'lease_expired' \
             AND finished_at IS NULL AND closed_at IS NOT NULL",
            2,
        )
        .await?;

        let repeated = persistence
            .recover_expired_leases(now, &ExecutionTenantScope::all())
            .await
            .map_err(|error| error.to_string())?;
        if repeated.requeued != 0 || repeated.completed != 0 || repeated.dead != 0 {
            return Err(format!("重复回收不应再次处理终态任务: {repeated:?}"));
        }
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_business_states_converge_across_failure_paths() {
    run_mysql_test("linked-failures", |database| async move {
        install_control_schema(&database).await?;
        seed_failure_path_imports(&database).await?;

        let persistence =
            application_ports::jobs::queue(ControlDatabaseCluster::single(database.clone()));
        let now = persistence
            .database_now()
            .await
            .map_err(|error| error.to_string())?;
        let failed = persistence
            .fail(FailJobCommand {
                job_id: 7301,
                claim_sequence: 1,
                worker_id: "active-worker",
                retry_at: now + Duration::minutes(1),
                error_message: "late failure",
                force_dead: false,
                now,
            })
            .await
            .map_err(|error| error.to_string())?;
        let dead_lettered = persistence
            .dead_letter(7302, 1, "active-worker", "missing handler", now)
            .await
            .map_err(|error| error.to_string())?;
        let deferred = persistence
            .defer_retryable_conflict(
                7303,
                1,
                "active-worker",
                now + Duration::minutes(1),
                "busy",
                now,
            )
            .await
            .map_err(|error| error.to_string())?;
        let terminal_retry = persistence
            .retry_dead("tenant-terminal", false, 7304, 1, now)
            .await
            .map_err(|error| error.to_string())?;
        let retry_clock = now.to_offset(offset!(+05:30));
        let eligible_retry = persistence
            .retry_dead("tenant-terminal", false, 7305, 1, retry_clock)
            .await
            .map_err(|error| error.to_string())?;

        for outcome in [failed, dead_lettered, deferred, terminal_retry] {
            if outcome != JobFailureOutcome::Completed {
                return Err(format!("权威完成态未收束为后台完成: {outcome:?}"));
            }
        }
        if !matches!(eligible_retry, JobFailureOutcome::Retried { .. }) {
            return Err(format!("明确失败态未进入人工重试: {eligible_retry:?}"));
        }
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job \
             WHERE id BETWEEN 7301 AND 7304 AND status = 'succeeded' \
             AND lease_owner IS NULL AND lease_until IS NULL",
            4,
        )
        .await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job \
             WHERE id = 7305 AND status = 'pending' AND completed_at IS NULL",
            1,
        )
        .await?;
        verify_retried_import_utc_timestamp(&database, now).await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job_attempt \
             WHERE job_id BETWEEN 7301 AND 7303 AND closed_at IS NOT NULL \
             AND ((job_id = 7301 AND outcome = 'failed') \
               OR (job_id = 7302 AND outcome = 'dead') \
               OR (job_id = 7303 AND outcome = 'deferred'))",
            3,
        )
        .await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job_attempt \
             WHERE job_id BETWEEN 7301 AND 7303 AND outcome = 'running'",
            0,
        )
        .await
    })
    .await;
}

async fn verify_retried_import_utc_timestamp(
    database: &DatabaseConnection,
    now: OffsetDateTime,
) -> Result<(), String> {
    let row = database
        .query_one_raw(Statement::from_string(
            DbBackend::MySql,
            "SELECT updated_at FROM sys_user_import_job \
             WHERE id = 7405 AND status = 'pending' AND completed_at IS NULL"
                .to_owned(),
        ))
        .await
        .map_err(|error| error.to_string())?
        .ok_or("未找到回到待执行状态的用户导入任务")?;
    let updated_at: PrimitiveDateTime = row
        .try_get("", "updated_at")
        .map_err(|error| error.to_string())?;
    if updated_at.assume_utc() != now {
        return Err(format!("导入重试时间应保留数据库 UTC 微秒：{updated_at}"));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linked_state_conflict_rolls_back_attempt_and_background_updates() {
    run_mysql_test("linked-attempt-conflict", |database| async move {
        install_control_schema(&database).await?;
        execute(
            &database,
            "INSERT INTO sys_background_job \
             (id, tenant_id, job_type, payload, status, priority, available_at, attempts, \
              claim_sequence, max_attempts, lease_owner, lease_until, created_at, updated_at) \
             VALUES (7501, 'tenant-terminal', 'system.user.import', \
              JSON_OBJECT('import_job_id', 'missing'), 'running', 0, UTC_TIMESTAMP(6), 1, 1, 3, \
              'active-worker', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR), \
              UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
        )
        .await?;
        execute(
            &database,
            "INSERT INTO sys_background_job_attempt \
             (job_id, sequence, available_at, started_at, outcome) \
             SELECT id, claim_sequence, available_at, updated_at, 'running' \
             FROM sys_background_job WHERE id = 7501",
        )
        .await?;
        let persistence =
            application_ports::jobs::queue(ControlDatabaseCluster::single(database.clone()));
        let now = persistence
            .database_now()
            .await
            .map_err(|error| error.to_string())?;
        if persistence
            .dead_letter(7501, 1, "active-worker", "missing link", now)
            .await
            .is_ok()
        {
            return Err("关联状态冲突必须失败关闭".into());
        }
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job \
             WHERE id = 7501 AND status = 'running' AND claim_sequence = 1 \
             AND lease_owner = 'active-worker'",
            1,
        )
        .await?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_background_job_attempt \
             WHERE job_id = 7501 AND sequence = 1 AND outcome = 'running' \
             AND finished_at IS NULL AND closed_at IS NULL",
            1,
        )
        .await
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retention_cutoff_keeps_utc_microseconds_and_excludes_current_run() {
    use ryframe_application::{
        TransactionAuditMode,
        ports::retention::{RetentionCutoff, RetentionResource, RetentionRunRecord},
    };
    use serde_json::json;
    use time::{UtcOffset, macros::datetime};

    run_mysql_test("retention-utc-cutoff", |database| async move {
        install_control_schema(&database).await?;
        let cluster = ControlDatabaseCluster::single(database.clone());
        let runs = application_ports::retention::run(cluster.clone());
        let cleanup = application_ports::retention::cleanup(cluster);
        let at = datetime!(2026-11-26 05:30:00.123456 +05:30);
        let older = at - Duration::microseconds(1);
        for (id, completed_at) in [
            (9001, Some(older)),
            (9002, Some(at)),
            (9003, Some(older)),
            (9004, None),
        ] {
            let status = if completed_at.is_some() {
                RetentionRunRecord::STATUS_SUCCEEDED
            } else {
                RetentionRunRecord::STATUS_PENDING
            };
            runs.insert_if_missing(RetentionRunRecord {
                id,
                background_job_id: id + 1000,
                trigger_kind: RetentionRunRecord::TRIGGER_MANUAL.into(),
                status: status.into(),
                policy_snapshot: json!({}),
                eligible_counts: json!({}),
                deleted_counts: json!({}),
                remaining_counts: json!({}),
                requested_by: None,
                error_summary: None,
                started_at: None,
                completed_at,
                created_at: at,
                updated_at: at,
            })
            .await
            .map_err(|error| error.to_string())?;
        }
        let transaction = runs.begin().await.map_err(|error| error.to_string())?;
        let run = transaction
            .lock_by_background_job(10004)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("未找到待执行的保留记录")?;
        let started = transaction
            .begin_run(run, at)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("保留记录未进入执行态")?;
        assert_eq!(started.started_at, Some(at.to_offset(UtcOffset::UTC)));
        transaction
            .commit(TransactionAuditMode::Skip)
            .await
            .map_err(|error| error.to_string())?;
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_data_retention_run WHERE id = 9004 \
             AND started_at = '2026-11-26 00:00:00.123456' AND completed_at IS NULL \
             AND updated_at = '2026-11-26 00:00:00.123456' AND status = 'running'",
            1,
        )
        .await?;
        let cutoff = RetentionCutoff {
            resource: RetentionResource::RetentionRuns,
            before: at,
        };
        let eligible = cleanup
            .preview(&[cutoff], Some(9003))
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(eligible.get("retention_runs"), Some(&1));
        let result = cleanup
            .cleanup_resource(cutoff, 10, 10, Some(9003))
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!((result.deleted, result.remaining), (1, 0));
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_data_retention_run WHERE id IN (9002, 9003, 9004)",
            3,
        )
        .await
    })
    .await;
}

async fn install_control_schema(database: &DatabaseConnection) -> Result<(), String> {
    for statement in migration::control_ddl_statements() {
        execute(database, statement).await?;
    }
    Ok(())
}

async fn seed_expired_terminal_imports(database: &DatabaseConnection) -> Result<(), String> {
    execute(
        database,
        "INSERT INTO sys_background_job \
         (id, tenant_id, job_type, payload, status, priority, available_at, attempts, claim_sequence, max_attempts, \
          lease_owner, lease_until, created_at, updated_at) VALUES \
         (7101, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7201'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 1, 3, 'expired-worker', \
          DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (7102, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7202'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 1, 3, 'expired-worker', \
          DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .await?;
    execute(
        database,
        "INSERT INTO sys_background_job_attempt \
         (job_id, sequence, available_at, started_at, outcome) \
         SELECT id, claim_sequence, available_at, updated_at, 'running' \
         FROM sys_background_job WHERE id IN (7101, 7102)",
    )
    .await?;
    execute(
        database,
        "INSERT INTO sys_user_import_job \
         (id, tenant_id, requester_user_id, background_job_id, idempotency_key_hash, source_file_id, \
          source_name_snapshot, source_sha256, duplicate_policy, status, completed_at, created_at, \
          updated_at) VALUES \
         (7201, 'tenant-terminal', 1, 7101, REPEAT('a', 64), 1, 'done.xlsx', REPEAT('b', 64), \
          'skip_existing', 'succeeded', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (7202, 'tenant-terminal', 1, 7102, REPEAT('c', 64), 2, 'failed.xlsx', REPEAT('d', 64), \
          'skip_existing', 'failed', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .await
}

async fn seed_failure_path_imports(database: &DatabaseConnection) -> Result<(), String> {
    execute(
        database,
        "INSERT INTO sys_background_job \
         (id, tenant_id, job_type, payload, status, priority, available_at, attempts, claim_sequence, max_attempts, \
          lease_owner, lease_until, created_at, updated_at, completed_at) VALUES \
         (7301, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7401'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 1, 3, 'active-worker', \
          DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), NULL), \
         (7302, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7402'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 1, 3, 'active-worker', \
          DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), NULL), \
         (7303, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7403'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 1, 3, 'active-worker', \
          DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), NULL), \
         (7304, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7404'), \
          'dead', 0, UTC_TIMESTAMP(6), 3, 3, 3, NULL, NULL, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), \
          UTC_TIMESTAMP(6)), \
         (7305, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7405'), \
          'dead', 0, UTC_TIMESTAMP(6), 3, 3, 3, NULL, NULL, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), \
          UTC_TIMESTAMP(6))",
    )
    .await?;
    execute(
        database,
        "INSERT INTO sys_background_job_attempt \
         (job_id, sequence, available_at, started_at, outcome) \
         SELECT id, claim_sequence, available_at, updated_at, 'running' \
         FROM sys_background_job WHERE id BETWEEN 7301 AND 7303",
    )
    .await?;
    execute(
        database,
        "INSERT INTO sys_user_import_job \
         (id, tenant_id, requester_user_id, background_job_id, idempotency_key_hash, source_file_id, \
          source_name_snapshot, source_sha256, duplicate_policy, status, completed_at, created_at, \
          updated_at) VALUES \
         (7401, 'tenant-terminal', 1, 7301, REPEAT('1', 64), 11, 'done.xlsx', REPEAT('a', 64), \
          'skip_existing', 'succeeded', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (7402, 'tenant-terminal', 1, 7302, REPEAT('2', 64), 12, 'cancelled.xlsx', REPEAT('b', 64), \
          'skip_existing', 'cancelled', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (7403, 'tenant-terminal', 1, 7303, REPEAT('3', 64), 13, 'partial.xlsx', REPEAT('c', 64), \
          'skip_existing', 'partial', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (7404, 'tenant-terminal', 1, 7304, REPEAT('4', 64), 14, 'terminal.xlsx', REPEAT('d', 64), \
          'skip_existing', 'succeeded', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (7405, 'tenant-terminal', 1, 7305, REPEAT('5', 64), 15, 'retry.xlsx', REPEAT('e', 64), \
          'skip_existing', 'failed', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
    )
    .await
}
