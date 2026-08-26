use chrono::{Duration, TimeZone, Utc};
use ryframe_application::{
    EnqueueJob,
    ports::{files::FileUploadRecord, jobs::JobScheduleRecord},
};
use ryframe_db::{
    application_ports::{
        files::{map_upload_model, map_upload_record},
        jobs::{database_enqueue, schedule_active, to_claimed_event, to_job_record},
    },
    entities::{background_job, job_schedule, outbox_event},
};
use sea_orm::ActiveValue::Set;
use serde_json::json;

fn upload_record() -> FileUploadRecord {
    let created_at = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    FileUploadRecord {
        id: 42,
        tenant_id: "tenant-a".to_owned(),
        original_name: "report.xlsx".to_owned(),
        storage_name: "opaque.xlsx".to_owned(),
        storage_path: "tenant-a/opaque.xlsx".to_owned(),
        bucket: "exports".to_owned(),
        file_url: "exports/tenant-a/opaque.xlsx".to_owned(),
        file_size: 123,
        content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            .to_owned(),
        file_sha256: "digest".to_owned(),
        upload_by: Some("operator".to_owned()),
        upload_status: "pending".to_owned(),
        reservation_token: Some("token".to_owned()),
        reservation_expires_at: Some(created_at + Duration::minutes(5)),
        del_flag: "0".to_owned(),
        created_at,
        updated_at: created_at,
    }
}

#[test]
fn upload_record_mapping_preserves_every_field() {
    assert_eq!(
        map_upload_record(map_upload_model(upload_record())),
        upload_record()
    );
}

#[test]
fn schedule_mapping_preserves_state_and_deletion() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let active = schedule_active(JobScheduleRecord {
        id: 42,
        tenant_id: "tenant-a".to_owned(),
        name: "清理任务".to_owned(),
        handler_key: "system.cleanup".to_owned(),
        cron_expression: "0 0 0 * * * *".to_owned(),
        timezone: "UTC".to_owned(),
        enabled: false,
        misfire_policy: "skip".to_owned(),
        concurrency_policy: "forbid".to_owned(),
        max_runtime_seconds: 600,
        next_run_at: None,
        last_run_at: Some(now),
        version: 5,
        created_at: now,
        updated_at: now,
        deleted: true,
    });

    assert_eq!(active.id, Set(42));
    assert_eq!(active.tenant_id, Set("tenant-a".to_owned()));
    assert_eq!(active.version, Set(5));
    assert_eq!(
        active.del_flag,
        Set(job_schedule::Model::DEL_FLAG_DELETED.to_owned())
    );
    assert_eq!(active.last_run_at, Set(Some(now)));
}

#[test]
fn enqueue_mapping_moves_every_command_field() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let command = database_enqueue(EnqueueJob {
        tenant_id: Some("tenant-a".into()),
        schedule_id: Some(8),
        scheduled_for: Some(now),
        max_runtime_seconds: Some(30),
        job_type: "test.job".into(),
        payload: json!({"id": "9"}),
        priority: 4,
        available_at: now,
        max_attempts: 5,
        dedupe_key: Some("dedupe".into()),
        traceparent: Some("traceparent".into()),
        tracestate: Some("tracestate".into()),
    });

    assert_eq!(command.tenant_id.as_deref(), Some("tenant-a"));
    assert_eq!(command.schedule_id, Some(8));
    assert_eq!(command.scheduled_for, Some(now));
    assert_eq!(command.max_runtime_seconds, Some(30));
    assert_eq!(command.job_type, "test.job");
    assert_eq!(command.payload, json!({"id": "9"}));
    assert_eq!(command.priority, 4);
    assert_eq!(command.available_at, now);
    assert_eq!(command.max_attempts, 5);
    assert_eq!(command.dedupe_key.as_deref(), Some("dedupe"));
    assert_eq!(command.traceparent.as_deref(), Some("traceparent"));
    assert_eq!(command.tracestate.as_deref(), Some("tracestate"));
}

#[test]
fn persistence_mapping_keeps_job_fields() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let record = to_job_record(background_job::Model {
        id: 9,
        tenant_id: Some("tenant-a".into()),
        schedule_id: Some(8),
        scheduled_for: Some(now),
        max_runtime_seconds: Some(30),
        job_type: "job.test".into(),
        payload: json!({"key": "value"}),
        status: background_job::Model::STATUS_RUNNING.into(),
        priority: 3,
        available_at: now,
        attempts: 2,
        max_attempts: 5,
        lease_owner: Some("worker-a".into()),
        lease_until: Some(now),
        dedupe_key: Some("dedupe".into()),
        traceparent: Some("trace".into()),
        tracestate: Some("state".into()),
        last_error: Some("error".into()),
        created_at: now,
        updated_at: now,
        completed_at: None,
    });

    assert_eq!(record.id, 9);
    assert_eq!(record.tenant_id.as_deref(), Some("tenant-a"));
    assert_eq!(record.schedule_id, Some(8));
    assert_eq!(record.job_type, "job.test");
    assert_eq!(record.attempts, 2);
    assert_eq!(record.max_attempts, 5);
    assert_eq!(record.last_error.as_deref(), Some("error"));
}

#[test]
fn claimed_event_mapping_keeps_worker_fields() {
    let now = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
    let event = outbox_event::Model {
        id: 41,
        tenant_id: Some("tenant-a".into()),
        event_type: "system.message.published".into(),
        aggregate_type: "message".into(),
        aggregate_id: "99".into(),
        payload: json!({"message_id": 99}),
        status: outbox_event::Model::STATUS_RUNNING.into(),
        available_at: now,
        attempts: 2,
        max_attempts: 5,
        lease_owner: Some("worker-a".into()),
        lease_until: Some(now + Duration::seconds(30)),
        dedupe_key: Some("message:99".into()),
        traceparent: Some("trace".into()),
        tracestate: Some("state".into()),
        last_error: None,
        published_at: None,
        created_at: now,
        updated_at: now,
    };

    let claimed = to_claimed_event(event);
    assert_eq!(claimed.id, 41);
    assert_eq!(claimed.tenant_id.as_deref(), Some("tenant-a"));
    assert_eq!(claimed.attempts, 2);
    assert_eq!(claimed.max_attempts, 5);
    assert_eq!(claimed.dedupe_key.as_deref(), Some("message:99"));
    assert_eq!(claimed.traceparent.as_deref(), Some("trace"));
    assert_eq!(claimed.tracestate.as_deref(), Some("state"));
}
