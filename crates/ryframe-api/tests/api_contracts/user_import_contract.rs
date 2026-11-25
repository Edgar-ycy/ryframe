use chrono::{DateTime, Utc};
use ryframe_api::dto::public_dto::UserImportJobVo;
use ryframe_application::{
    ports::users::UserImportJobRecord, system::identity::UserImportJobVo as ServiceJob,
};
use ryframe_kernel::AppError;
use serde_json::{json, to_value};
use utoipa::PartialSchema;

#[test]
fn user_import_job_timestamps_preserve_utc_json_and_nullable_schema() {
    let created_at = DateTime::from_timestamp(-2, 0).unwrap();
    let started_at = DateTime::from_timestamp(-1, 120_000_000).unwrap();
    let completed_at = DateTime::from_timestamp(0, 123_400_000).unwrap();
    let updated_at = DateTime::from_timestamp(0, 123_456_700).unwrap();

    for (started_at, completed_at) in [
        (None, None),
        (Some(started_at), None),
        (Some(started_at), Some(completed_at)),
    ] {
        let mut record = job_record(created_at);
        record.started_at = started_at;
        record.completed_at = completed_at;
        record.updated_at = updated_at;
        let job = ServiceJob::try_from(record).unwrap();
        let service_json = to_value(&job).unwrap();
        let response = to_value(UserImportJobVo::from(job)).unwrap();
        assert_eq!(response, service_json);
        for (field, expected) in [
            ("started_at", json!(started_at)),
            ("completed_at", json!(completed_at)),
            ("created_at", json!(created_at)),
            ("updated_at", json!(updated_at)),
        ] {
            assert_eq!(response[field], expected, "{field}");
        }
    }

    let schema = to_value(UserImportJobVo::schema()).unwrap();
    let required = schema["required"].as_array().unwrap();
    for field in ["started_at", "completed_at"] {
        assert_eq!(
            schema["properties"][field],
            json!({"type":["string","null"],"format":"date-time"})
        );
        assert!(!required.contains(&json!(field)));
    }
    for field in ["created_at", "updated_at"] {
        assert_eq!(
            schema["properties"][field],
            json!({"type":"string","format":"date-time"})
        );
        assert!(required.contains(&json!(field)));
    }
}

#[test]
fn user_import_job_rejects_an_unrepresentable_timestamp_with_its_field() {
    let mut record = job_record(DateTime::UNIX_EPOCH);
    record.completed_at = Some(DateTime::from_timestamp(-1, 1_123_456_000).unwrap());
    assert!(matches!(
        ServiceJob::try_from(record),
        Err(AppError::Internal(message)) if message.contains("completed_at")
    ));
}

fn job_record(now: DateTime<Utc>) -> UserImportJobRecord {
    UserImportJobRecord {
        id: 42,
        tenant_id: "tenant-a".into(),
        requester_user_id: 9,
        background_job_id: 19,
        idempotency_key_hash: "a".repeat(64),
        source_file_id: 22,
        source_name_snapshot: "users.xlsx".into(),
        source_sha256: "b".repeat(64),
        duplicate_policy: "skip".into(),
        status: UserImportJobRecord::STATUS_PENDING.into(),
        total_rows: 0,
        processed_rows: 0,
        success_count: 0,
        skipped_count: 0,
        failure_count: 0,
        cancel_requested: false,
        error_report_file_id: None,
        last_error: None,
        started_at: None,
        completed_at: None,
        created_at: now,
        updated_at: now,
    }
}
