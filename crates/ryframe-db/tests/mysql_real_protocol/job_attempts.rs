use super::{ControlDatabaseCluster, application_ports, execute, require_count, run_mysql_test};
use chrono::{DateTime, Duration, Utc};
use ryframe_application::ports::jobs::{
    BackgroundJobPersistencePort, ClaimedJobRecord, ExecutionTenantScope, FailJobCommand,
    JobFailureOutcome,
};
use ryframe_db::entities::{background_job, background_job_attempt};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use std::sync::Arc;

struct Fixture {
    database: DatabaseConnection,
    queue: Arc<dyn BackgroundJobPersistencePort>,
    now: DateTime<Utc>,
}

impl Fixture {
    async fn new(database: DatabaseConnection) -> Result<Self, String> {
        ryframe_db::migration::up(&database)
            .await
            .map_err(|error| error.to_string())?;
        let queue =
            application_ports::jobs::queue(ControlDatabaseCluster::single(database.clone()));
        let now = queue
            .database_now()
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            database,
            queue,
            now,
        })
    }

    async fn job(&self, id: i64, kind: &str, maximum: i32) -> Result<(), String> {
        execute(&self.database, &format!(
            "INSERT INTO sys_background_job (id,tenant_id,job_type,payload,status,priority,available_at,max_attempts,created_at,updated_at) \
             VALUES ({id},'attempt-tenant','{kind}','{{}}','pending',0,'{}',{maximum},'{}','{}')",
            self.now.format("%Y-%m-%d %H:%M:%S%.6f"),
            self.now.format("%Y-%m-%d %H:%M:%S%.6f"),
            self.now.format("%Y-%m-%d %H:%M:%S%.6f")
        ))
        .await
    }

    async fn claim(&self, second: i64) -> Result<ClaimedJobRecord, String> {
        self.queue
            .claim_next(
                "attempt-worker",
                Duration::seconds(60),
                self.at(second),
                &ExecutionTenantScope::all(),
            )
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "领取任务为空".into())
    }

    fn at(&self, second: i64) -> DateTime<Utc> {
        self.now + Duration::seconds(second)
    }

    async fn attempts(&self, id: i64) -> Result<Vec<background_job_attempt::Model>, String> {
        background_job_attempt::Entity::find()
            .filter(background_job_attempt::Column::JobId.eq(id))
            .order_by_asc(background_job_attempt::Column::Sequence)
            .all(&self.database)
            .await
            .map_err(|error| error.to_string())
    }

    async fn row(&self, id: i64) -> Result<background_job::Model, String> {
        background_job::Entity::find_by_id(id)
            .one(&self.database)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "任务记录丢失".into())
    }
}

fn assert_retry_history(f: &Fixture, rows: &[background_job_attempt::Model]) {
    assert_eq!(
        rows.iter()
            .map(|row| row.outcome.as_str())
            .collect::<Vec<_>>(),
        ["failed", "deferred", "dead", "succeeded"]
    );
    for (row, (sequence, available, start, finish)) in rows.iter().zip([
        (1, 0, 1, 3),
        (2, 10, 11, 12),
        (3, 20, 21, 22),
        (4, 30, 31, 33),
    ]) {
        assert_eq!(row.sequence, sequence);
        assert_eq!(row.available_at, f.at(available));
        assert_eq!(row.started_at, f.at(start));
        assert_eq!(row.finished_at, Some(f.at(finish)));
        assert_eq!(row.closed_at, row.finished_at);
    }
}

async fn exercise_retry_history(f: &Fixture) -> Result<(), String> {
    f.job(101, "attempt.test", 2).await?;
    let first = f.claim(1).await?;
    assert_eq!((first.id, first.claim_sequence), (101, 1));
    assert!(
        f.queue
            .renew_lease(
                101,
                first.claim_sequence,
                "attempt-worker",
                Duration::seconds(60),
                f.at(2),
            )
            .await
            .map_err(|error| error.to_string())?
    );
    assert_eq!(f.attempts(101).await?[0].started_at, f.at(1));
    f.queue
        .fail(FailJobCommand {
            job_id: 101,
            claim_sequence: first.claim_sequence,
            worker_id: "attempt-worker",
            retry_at: f.at(10),
            error_message: "fixture failure",
            force_dead: false,
            now: f.at(3),
        })
        .await
        .map_err(|error| error.to_string())?;
    let second = f.claim(11).await?;
    assert_eq!((second.id, second.claim_sequence), (101, 2));
    f.queue
        .defer_retryable_conflict(
            101,
            second.claim_sequence,
            "attempt-worker",
            f.at(20),
            "fixture conflict",
            f.at(12),
        )
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(f.row(101).await?.attempts, 1);
    let third = f.claim(21).await?;
    assert_eq!((third.id, third.claim_sequence), (101, 3));
    f.queue
        .dead_letter(
            101,
            third.claim_sequence,
            "attempt-worker",
            "fixture dead",
            f.at(22),
        )
        .await
        .map_err(|error| error.to_string())?;
    assert!(matches!(
        f.queue
            .retry_dead("attempt-tenant", false, 101, 1, f.at(30))
            .await
            .map_err(|error| error.to_string())?,
        JobFailureOutcome::Retried { .. }
    ));
    assert_eq!(f.row(101).await?.attempts, 0);
    assert_eq!(f.row(101).await?.claim_sequence, 3);
    let fourth = f.claim(31).await?;
    assert_eq!((fourth.id, fourth.claim_sequence), (101, 4));
    assert!(
        !f.queue
            .complete(101, third.claim_sequence, "attempt-worker", f.at(32))
            .await
            .map_err(|error| error.to_string())?
    );
    assert!(f.attempts(101).await?[3].closed_at.is_none());
    assert!(
        f.queue
            .complete(101, fourth.claim_sequence, "attempt-worker", f.at(33))
            .await
            .map_err(|error| error.to_string())?
    );
    assert_retry_history(f, &f.attempts(101).await?);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_attempts_preserve_retries_defer_and_manual_budget_reset() {
    run_mysql_test("job_attempts", |database| async move {
        let f = Fixture::new(database).await?;
        exercise_retry_history(&f).await
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_worker_stale_sequence_cannot_mutate_a_reclaimed_job() {
    run_mysql_test("job_attempt_aba", |database| async move {
        let f = Fixture::new(database).await?;
        f.job(151, "attempt.test", 3).await?;
        let stale = f.claim(1).await?;
        f.queue
            .recover_expired_leases(f.at(62), &ExecutionTenantScope::all())
            .await
            .map_err(|error| error.to_string())?;
        let current = f.claim(63).await?;
        assert_eq!((stale.claim_sequence, current.claim_sequence), (1, 2));

        assert!(
            !f.queue
                .renew_lease(
                    151,
                    stale.claim_sequence,
                    "attempt-worker",
                    Duration::seconds(60),
                    f.at(64),
                )
                .await
                .map_err(|error| error.to_string())?
        );
        assert!(
            !f.queue
                .complete(151, stale.claim_sequence, "attempt-worker", f.at(64))
                .await
                .map_err(|error| error.to_string())?
        );
        let failed = f
            .queue
            .fail(FailJobCommand {
                job_id: 151,
                claim_sequence: stale.claim_sequence,
                worker_id: "attempt-worker",
                retry_at: f.at(70),
                error_message: "stale failure",
                force_dead: false,
                now: f.at(64),
            })
            .await
            .map_err(|error| error.to_string())?;
        let deferred = f
            .queue
            .defer_retryable_conflict(
                151,
                stale.claim_sequence,
                "attempt-worker",
                f.at(70),
                "stale defer",
                f.at(64),
            )
            .await
            .map_err(|error| error.to_string())?;
        let dead = f
            .queue
            .dead_letter(
                151,
                stale.claim_sequence,
                "attempt-worker",
                "stale dead",
                f.at(64),
            )
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(failed, JobFailureOutcome::LeaseLost);
        assert_eq!(deferred, JobFailureOutcome::LeaseLost);
        assert_eq!(dead, JobFailureOutcome::LeaseLost);

        let row = f.row(151).await?;
        assert_eq!(row.status, background_job::Model::STATUS_RUNNING);
        assert_eq!(row.claim_sequence, current.claim_sequence);
        assert_eq!(row.lease_owner.as_deref(), Some("attempt-worker"));
        let rows = f.attempts(151).await?;
        assert_eq!(rows[0].outcome, "lease_expired");
        assert_eq!(rows[1].outcome, "running");
        assert!(rows[1].closed_at.is_none());
        assert!(
            f.queue
                .complete(151, current.claim_sequence, "attempt-worker", f.at(65))
                .await
                .map_err(|error| error.to_string())?
        );
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_terminal_writers_close_exactly_one_attempt() {
    run_mysql_test("job_attempt_race", |database| async move {
        let f = Fixture::new(database).await?;
        f.job(181, "attempt.test", 1).await?;
        let claim = f.claim(1).await?;
        let complete_queue = f.queue.clone();
        let fail_queue = f.queue.clone();
        let now = f.at(2);
        let (completed, failed) = tokio::join!(
            complete_queue.complete(181, claim.claim_sequence, "attempt-worker", now),
            fail_queue.fail(FailJobCommand {
                job_id: 181,
                claim_sequence: claim.claim_sequence,
                worker_id: "attempt-worker",
                retry_at: now,
                error_message: "race failure",
                force_dead: true,
                now,
            })
        );
        let completed = completed.map_err(|error| error.to_string())?;
        let failed = failed.map_err(|error| error.to_string())?;
        assert!(
            (completed && failed == JobFailureOutcome::LeaseLost)
                || (!completed && failed == JobFailureOutcome::Dead)
        );
        let attempts = f.attempts(181).await?;
        assert_eq!(attempts.len(), 1);
        assert!(matches!(attempts[0].outcome.as_str(), "succeeded" | "dead"));
        assert!(attempts[0].closed_at.is_some());
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_attempts_reject_stale_completion_and_reactivate_without_erasing_history() {
    run_mysql_test("job_expired", |database| async move {
        let f = Fixture::new(database).await?;
        f.job(201, "attempt.test", 2).await?;
        let first = f.claim(1).await?;
        let recovered = f
            .queue
            .recover_expired_leases(f.at(62), &ExecutionTenantScope::all())
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!((recovered.requeued, recovered.dead), (1, 0));
        let second = f.claim(64).await?;
        assert!(
            !f.queue
                .complete(201, first.claim_sequence, "attempt-worker", f.at(65))
                .await
                .map_err(|error| error.to_string())?
        );
        assert!(f.attempts(201).await?[1].closed_at.is_none());
        let recovered = f
            .queue
            .recover_expired_leases(f.at(125), &ExecutionTenantScope::all())
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!((recovered.requeued, recovered.dead), (0, 1));
        assert_eq!(second.claim_sequence, 2);
        for row in f.attempts(201).await? {
            assert_eq!(row.outcome, "lease_expired");
            assert!(row.finished_at.is_none());
            assert!(row.closed_at.is_some());
        }

        f.job(202, "attempt.linked", 2).await?;
        execute(
            &f.database,
            r#"UPDATE sys_background_job SET payload='{"resource_id":"901"}' WHERE id=202"#,
        )
        .await?;
        let first = f.claim(130).await?;
        let transaction = sea_orm::TransactionTrait::begin(&f.database)
            .await
            .map_err(|error| error.to_string())?;
        assert!(
            ryframe_db::repositories::BackgroundJobRepository
                .reactivate_linked_in_txn(
                    &transaction,
                    202,
                    "attempt.linked",
                    "resource_id",
                    901,
                    f.at(191),
                )
                .await
                .map_err(|error| error.to_string())?
        );
        transaction
            .commit()
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(f.attempts(202).await?[0].outcome, "lease_expired");
        let second = f.claim(192).await?;
        assert_eq!((first.claim_sequence, second.claim_sequence), (1, 2));
        Ok(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_attempt_recovery_drains_multiple_bounded_batches() {
    run_mysql_test("attempt_batches", |database| async move {
        let f = Fixture::new(database).await?;
        let time = f.now.format("%Y-%m-%d %H:%M:%S%.6f");
        let values = (1000..1501)
            .map(|id| {
                format!(
                    "({id},'attempt-tenant','attempt.test','{{}}','running',0,'{time}',1,1,2,'expired','{time}','{time}','{time}')"
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        execute(&f.database, &format!(
            "INSERT INTO sys_background_job (id,tenant_id,job_type,payload,status,priority,available_at,attempts,claim_sequence,max_attempts,lease_owner,lease_until,created_at,updated_at) VALUES {values}"
        )).await?;
        execute(&f.database, "INSERT INTO sys_background_job_attempt (job_id,sequence,available_at,started_at,outcome) SELECT id,1,available_at,available_at,'running' FROM sys_background_job WHERE id BETWEEN 1000 AND 1500")
            .await?;
        let first = f.queue.recover_expired_leases(f.at(1), &ExecutionTenantScope::all()).await.map_err(|error| error.to_string())?;
        let second = f.queue.recover_expired_leases(f.at(2), &ExecutionTenantScope::all()).await.map_err(|error| error.to_string())?;
        assert_eq!((first.requeued, second.requeued), (500, 1));
        require_count(&f.database, "SELECT COUNT(*) AS value FROM sys_background_job_attempt WHERE outcome='lease_expired' AND finished_at IS NULL AND closed_at IS NOT NULL", 501).await
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_attempt_constraints_and_owned_job_deletion_preserve_history_integrity() {
    run_mysql_test("attempt_checks", |database| async move {
        let f = Fixture::new(database).await?;
        f.job(301, "attempt.test", 1).await?;
        f.job(302, "attempt.test", 1).await?;
        let first = f.claim(1).await?;
        for sql in [
            "UPDATE sys_background_job_attempt SET closed_at=started_at WHERE job_id=301",
            "UPDATE sys_background_job_attempt SET outcome='succeeded',finished_at=started_at,closed_at=NULL WHERE job_id=301",
            "UPDATE sys_background_job_attempt SET outcome='lease_expired',finished_at=started_at,closed_at=started_at WHERE job_id=301",
            "UPDATE sys_background_job SET claim_sequence=-1 WHERE id=301",
            "UPDATE sys_background_job SET attempts=2 WHERE id=301",
            "UPDATE sys_background_job SET max_attempts=101 WHERE id=301",
        ] {
            assert!(
                execute(&f.database, sql).await.is_err(),
                "非法任务或尝试事实必须被约束拒绝"
            );
        }
        assert!(f.queue.complete(301, first.claim_sequence, "attempt-worker", f.at(3)).await.map_err(|error| error.to_string())?);
        let second = f.claim(4).await?;
        assert!(f.queue.complete(302, second.claim_sequence, "attempt-worker", f.at(5)).await.map_err(|error| error.to_string())?);
        execute(&f.database, "DELETE FROM sys_background_job WHERE id=301").await?;
        require_count(&f.database, "SELECT COUNT(*) AS value FROM sys_background_job_attempt WHERE job_id=301", 0).await?;
        require_count(&f.database, "SELECT COUNT(*) AS value FROM sys_background_job_attempt WHERE job_id=302", 1).await
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attempt_write_failure_rolls_back_claim_and_completion() {
    run_mysql_test("attempt_atomic", |database| async move {
        let f = Fixture::new(database).await?;
        f.job(401, "attempt.test", 1).await?;
        execute(&f.database, "INSERT INTO sys_background_job_attempt (job_id,sequence,available_at,started_at,outcome) SELECT id,1,available_at,available_at,'running' FROM sys_background_job WHERE id=401")
            .await?;
        assert!(f.claim(1).await.is_err());
        let row = f.row(401).await?;
        assert_eq!((row.status.as_str(), row.attempts, row.claim_sequence), ("pending", 0, 0));
        execute(&f.database, "DELETE FROM sys_background_job_attempt WHERE job_id=401").await?;
        let claim = f.claim(2).await?;
        execute(&f.database, "DELETE FROM sys_background_job_attempt WHERE job_id=401").await?;
        assert!(f.queue.complete(401, claim.claim_sequence, "attempt-worker", f.at(3)).await.is_err());
        assert_eq!(f.row(401).await?.status, "running");
        Ok(())
    }).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_business_job_keeps_its_state_when_worker_finishes() {
    run_mysql_test("attempt_cancel", |database| async move {
        let f = Fixture::new(database).await?;
        f.job(501, "system.user.import", 3).await?;
        execute(&f.database, "INSERT INTO sys_user_import_job (id,tenant_id,requester_user_id,background_job_id,idempotency_key_hash,source_file_id,source_name_snapshot,source_sha256,status,cancel_requested,created_at,updated_at) \
            SELECT 601,'attempt-tenant',1,id,REPEAT('a',64),1,'fixture.xlsx',REPEAT('b',64),'cancelled',1,created_at,updated_at FROM sys_background_job WHERE id=501")
            .await?;
        let claim = f.claim(1).await?;
        assert!(f.queue.complete(501, claim.claim_sequence, "attempt-worker", f.at(2)).await.map_err(|error| error.to_string())?);
        assert_eq!(f.attempts(501).await?[0].outcome, "succeeded");
        require_count(&f.database, "SELECT COUNT(*) AS value FROM sys_user_import_job WHERE id=601 AND status='cancelled' AND cancel_requested=1", 1).await
    }).await;
}
