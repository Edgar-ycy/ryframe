use ryframe_application::{
    EnqueueJob, TransactionAuditMode,
    ports::{
        export::{
            CompleteExportArtifact, CreateExportRecord, ExportArtifactFileDraft,
            ExportRequesterRecord, ExportStartDecision,
        },
        jobs::{ClaimedJobRecord, ExecutionTenantScope, JobFailureOutcome},
    },
    system::operations::{
        EXPORT_JOB_TYPE, EXPORT_REQUEST_VERSION, ExportJobPayload, StoredExportRequest,
        export_requester_snapshot,
    },
};
use ryframe_db::{ControlDatabaseCluster, application_ports, migration};
use ryframe_kernel::AppError;
use sea_orm::DatabaseConnection;
use serde_json::json;
use time::{Duration, OffsetDateTime, UtcOffset, macros::datetime};

use super::{execute, install_test_database_id_generator, require_count, run_mysql_test};

const TENANT: &str = "export-clock";
const WORKER: &str = "export-clock-worker";
const REQUESTER: i64 = 7;
const SNAPSHOT: OffsetDateTime = datetime!(2026-08-21 06:30:01.123456 +05:30);
const FINISHED: OffsetDateTime = datetime!(2026-08-21 06:30:03.123456 +05:30);
const EXPIRES: OffsetDateTime = datetime!(2026-08-22 01:00:03 UTC);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_lifecycle_preserves_snapshot_and_utc_boundaries() {
    run_mysql_test("export-lifecycle-utc", |database| async move {
        install_test_database_id_generator()?;
        for statement in migration::control_ddl_statements() {
            execute(&database, statement).await?;
        }
        execute(
            &database,
            "INSERT INTO sys_tenant(id,tenant_id,name,status) \
             VALUES (90030,'export-clock','Export clock','enabled')",
        )
        .await?;
        let cluster = ControlDatabaseCluster::single(database.clone());
        let (succeeded, claim) = start_export(&cluster).await?;
        check_snapshot(&database, &cluster, &succeeded).await?;
        finish_with_artifact(&cluster, succeeded.id).await?;
        check_expiry_and_deletion_lease(&database, &cluster, succeeded.id, claim).await?;
        let (failed, claim) = start_export(&cluster).await?;
        check_terminal_failure(&database, &cluster, failed.id, claim).await
    })
    .await;
}

async fn start_export(
    cluster: &ControlDatabaseCluster,
) -> Result<(ExportRequesterRecord, ClaimedJobRecord), String> {
    let requests = application_ports::export::request(cluster.clone());
    let transaction = requests.begin().await.map_err(|error| error.to_string())?;
    let payload =
        serde_json::to_value(ExportJobPayload::new("roles")).map_err(|error| error.to_string())?;
    assert_eq!(payload, json!({"resource": "roles", "request_version": 2}));
    let background_job_id = transaction
        .enqueue_job(
            EnqueueJob {
                tenant_id: Some(TENANT.into()),
                schedule_id: None,
                scheduled_for: Some(SNAPSHOT),
                max_runtime_seconds: None,
                job_type: EXPORT_JOB_TYPE.into(),
                payload,
                priority: 0,
                available_at: SNAPSHOT,
                max_attempts: 1,
                dedupe_key: None,
                traceparent: None,
                tracestate: None,
            },
            SNAPSHOT,
        )
        .await
        .map_err(|error| error.to_string())?;
    let authorization_fingerprint = "a".repeat(64);
    let export = transaction
        .create_export(
            CreateExportRecord {
                tenant_id: TENANT.into(),
                requester_id: REQUESTER,
                resource: "roles".into(),
                background_job_id,
                request_params: json!({
                    "request_version": 2,
                    "selection": {
                        "resource": "roles",
                        "filter": {"name": "ops", "code": null, "status": "0"}
                    },
                    "authorization_fingerprint": authorization_fingerprint,
                    "snapshot_at": "2026-08-21T01:00:01.123456Z",
                    "upper_id": 90,
                    "matched_rows": 2
                }),
                request_version: i32::from(EXPORT_REQUEST_VERSION),
                permission_code: "system:role:export".into(),
                authorization_fingerprint,
                request_fingerprint: "b".repeat(64),
                snapshot_at: SNAPSHOT,
                upper_id: 90,
                matched_rows: 2,
            },
            SNAPSHOT,
        )
        .await
        .map_err(|error| error.to_string())?;
    transaction
        .commit(TransactionAuditMode::Skip)
        .await
        .map_err(|error| error.to_string())?;
    let queue = application_ports::jobs::queue(cluster.clone());
    let claim = queue
        .claim_next(
            WORKER,
            Duration::minutes(1),
            SNAPSHOT + Duration::seconds(1),
            &ExecutionTenantScope::all(),
        )
        .await
        .map_err(|error| error.to_string())?
        .ok_or("导出后台任务未领取")?;
    assert_eq!(claim.id, background_job_id);
    let execution = application_ports::export::execution(cluster.clone());
    let transaction = execution.begin().await.map_err(|error| error.to_string())?;
    assert_eq!(
        transaction
            .try_start(export.id, TENANT, 2, SNAPSHOT + Duration::seconds(1))
            .await
            .map_err(|error| error.to_string())?,
        ExportStartDecision::Started
    );
    transaction
        .commit(TransactionAuditMode::Skip)
        .await
        .map_err(|error| error.to_string())?;
    Ok((export, claim))
}

async fn check_snapshot(
    database: &DatabaseConnection,
    cluster: &ControlDatabaseCluster,
    export: &ExportRequesterRecord,
) -> Result<(), String> {
    let requester = application_ports::export::requester(cluster.clone());
    let reread = requester
        .find(TENANT, REQUESTER, export.id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("导出请求快照未回读")?;
    assert_eq!(reread.snapshot_at, SNAPSHOT);
    assert_eq!(reread.snapshot_at.offset(), UtcOffset::UTC);
    assert_eq!(reread.request_params, export.request_params);
    let stored: StoredExportRequest =
        serde_json::from_value(reread.request_params.clone()).map_err(|error| error.to_string())?;
    stored
        .validate_persisted_snapshot(export_requester_snapshot(&reread))
        .map_err(|error| error.to_string())?;
    require_count(
        database,
        &format!(
            "SELECT COUNT(*) AS value FROM sys_export_job AS e \
             JOIN sys_background_job AS j ON j.id=e.background_job_id \
             WHERE e.id={} AND e.snapshot_at='2026-08-21 01:00:01.123456' \
             AND e.created_at='2026-08-21 01:00:01' \
             AND JSON_UNQUOTE(JSON_EXTRACT(e.request_params,'$.snapshot_at'))='2026-08-21T01:00:01.123456Z' \
             AND j.available_at=e.snapshot_at AND j.scheduled_for=e.snapshot_at",
            export.id
        ),
        1,
    )
    .await
}

async fn finish_with_artifact(
    cluster: &ControlDatabaseCluster,
    export_id: i64,
) -> Result<(), String> {
    let artifact = application_ports::export::artifact(cluster.clone());
    let transaction = artifact.begin().await.map_err(|error| error.to_string())?;
    let job = transaction
        .lock_export(export_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("无法锁定导出结果")?;
    assert_eq!(job.status, "running");
    let file = transaction
        .insert_ready_file(
            TENANT,
            ExportArtifactFileDraft {
                id: 90101,
                file_name: "roles.xlsx".into(),
                storage_path: "export-clock/roles.xlsx".into(),
                bucket: "exports".into(),
                file_url: "exports/export-clock/roles.xlsx".into(),
                file_size: 128,
                content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                    .into(),
                sha256: "c".repeat(64),
                uploaded_by: "fixture".into(),
                created_at: SNAPSHOT,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    assert!(
        transaction
            .mark_succeeded(CompleteExportArtifact {
                export_id,
                file_id: file.id,
                file_name: file.file_name,
                content_type: file.content_type,
                file_size: file.file_size,
                expires_at: FINISHED + Duration::hours(24),
                completed_at: FINISHED,
            })
            .await
            .map_err(|error| error.to_string())?
    );
    transaction
        .commit(TransactionAuditMode::Skip)
        .await
        .map_err(|error| error.to_string())
}

async fn check_expiry_and_deletion_lease(
    database: &DatabaseConnection,
    cluster: &ControlDatabaseCluster,
    export_id: i64,
    claim: ClaimedJobRecord,
) -> Result<(), String> {
    let cleanup = application_ports::export::cleanup(cluster.clone());
    let before = EXPIRES - Duration::microseconds(1);
    assert!(
        cleanup
            .list_expired(before, None, 5)
            .await
            .map_err(|error| error.to_string())?
            .is_empty()
    );
    let expired = cleanup
        .list_expired(EXPIRES, None, 5)
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].id, export_id);
    assert_eq!(expired[0].expires_at, Some(EXPIRES));
    require_count(
        database,
        &format!(
            "SELECT COUNT(*) AS value FROM sys_export_job AS e JOIN sys_file AS f ON f.id=e.result_file_id \
             WHERE e.id={export_id} AND e.status='succeeded' AND f.upload_status='ready' \
             AND e.completed_at='2026-08-21 01:00:03' AND e.expires_at='2026-08-22 01:00:03' \
             AND f.created_at='2026-08-21 01:00:01'"
        ),
        1,
    ).await?;

    let execution = application_ports::export::execution(cluster.clone());
    let deadline = execution
        .find_background_lease(claim.id)
        .await
        .map_err(|error| error.to_string())?
        .and_then(|lease| lease.lease_until)
        .ok_or("导出后台租约未回读")?;
    assert_eq!(deadline, SNAPSHOT + Duration::seconds(61));
    let deletion = application_ports::export::deletion(cluster.clone());
    let transaction = deletion.begin().await.map_err(|error| error.to_string())?;
    assert!(matches!(
        transaction.mark_delete_pending(TENANT, REQUESTER, &[export_id], deadline - Duration::microseconds(1)).await,
        Err(AppError::Conflict(message)) if message.contains("仍被 Worker 持有")
    ));
    assert_eq!(
        transaction
            .mark_delete_pending(TENANT, REQUESTER, &[export_id], deadline)
            .await
            .map_err(|error| error.to_string())?,
        1
    );
    transaction
        .rollback()
        .await
        .map_err(|error| error.to_string())?;
    let queue = application_ports::jobs::queue(cluster.clone());
    assert!(
        queue
            .complete(claim.id, claim.claim_sequence, WORKER, FINISHED)
            .await
            .map_err(|error| error.to_string())?
    );
    Ok(())
}

async fn check_terminal_failure(
    database: &DatabaseConnection,
    cluster: &ControlDatabaseCluster,
    export_id: i64,
    claim: ClaimedJobRecord,
) -> Result<(), String> {
    let queue = application_ports::jobs::queue(cluster.clone());
    let outcome = queue
        .dead_letter(
            claim.id,
            claim.claim_sequence,
            WORKER,
            "fixture failure",
            FINISHED,
        )
        .await
        .map_err(|error| error.to_string())?;
    assert_eq!(outcome, JobFailureOutcome::Dead);
    require_count(
        database,
        &format!(
            "SELECT COUNT(*) AS value FROM sys_export_job AS e \
             JOIN sys_background_job AS j ON j.id=e.background_job_id \
             WHERE e.id={export_id} AND j.status='dead' AND e.status='failed' \
             AND e.active_request_fingerprint IS NULL AND j.lease_until IS NULL \
             AND e.completed_at='2026-08-21 01:00:03' \
             AND j.completed_at='2026-08-21 01:00:03.123456'"
        ),
        1,
    )
    .await
}
