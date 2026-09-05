use ryframe_application::ports::jobs::{ExecutionTenantScope, FailJobCommand, JobFailureOutcome};
use ryframe_db::{ControlDatabaseCluster, application_ports, migration};
use sea_orm::DatabaseConnection;

use super::mysql::{execute, require_count, run_mysql_test};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_terminal_linked_jobs_do_not_requeue() {
    run_mysql_test("linked-terminal", |database| async move {
        install_control_schema(&database).await?;
        seed_expired_terminal_imports(&database).await?;

        let persistence =
            application_ports::jobs::queue(ControlDatabaseCluster::single(database.clone()));
        let now = chrono::Utc::now();
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
                worker_id: "active-worker",
                retry_at: now + chrono::Duration::minutes(1),
                error_message: "late failure",
                force_dead: false,
                now,
            })
            .await
            .map_err(|error| error.to_string())?;
        let dead_lettered = persistence
            .dead_letter(7302, "active-worker", "missing handler", now)
            .await
            .map_err(|error| error.to_string())?;
        let deferred = persistence
            .defer_retryable_conflict(
                7303,
                "active-worker",
                now + chrono::Duration::minutes(1),
                "busy",
                now,
            )
            .await
            .map_err(|error| error.to_string())?;
        let terminal_retry = persistence
            .retry_dead("tenant-terminal", false, 7304, 1, now)
            .await
            .map_err(|error| error.to_string())?;
        let eligible_retry = persistence
            .retry_dead("tenant-terminal", false, 7305, 1, now)
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
        require_count(
            &database,
            "SELECT COUNT(*) AS value FROM sys_user_import_job \
             WHERE id = 7405 AND status = 'pending' AND completed_at IS NULL",
            1,
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
         (id, tenant_id, job_type, payload, status, priority, available_at, attempts, max_attempts, \
          lease_owner, lease_until, created_at, updated_at) VALUES \
         (7101, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7201'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 3, 'expired-worker', \
          DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6)), \
         (7102, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7202'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 3, 'expired-worker', \
          DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))",
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
         (id, tenant_id, job_type, payload, status, priority, available_at, attempts, max_attempts, \
          lease_owner, lease_until, created_at, updated_at, completed_at) VALUES \
         (7301, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7401'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 3, 'active-worker', \
          DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), NULL), \
         (7302, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7402'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 3, 'active-worker', \
          DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), NULL), \
         (7303, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7403'), \
          'running', 0, UTC_TIMESTAMP(6), 1, 3, 'active-worker', \
          DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 1 HOUR), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), NULL), \
         (7304, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7404'), \
          'dead', 0, UTC_TIMESTAMP(6), 3, 3, NULL, NULL, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), \
          UTC_TIMESTAMP(6)), \
         (7305, 'tenant-terminal', 'system.user.import', JSON_OBJECT('import_job_id', '7405'), \
          'dead', 0, UTC_TIMESTAMP(6), 3, 3, NULL, NULL, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), \
          UTC_TIMESTAMP(6))",
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
