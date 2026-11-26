use ryframe_api::{openapi::ApiDoc, permission_catalog::*};

#[test]
fn generated_post_router_composes_with_manual_export_extension() {
    use std::sync::Arc;

    use axum::{Router, routing::post};
    use ryframe_application::generated::post::{PostFakePersistence, PostService};
    use ryframe_kernel::PaginationPolicy;

    let service = Arc::new(PostService::new(Arc::new(PostFakePersistence::default())));
    let export = Router::new().route("/exports", post(|| async {}));

    let _router: Router = Router::new()
        .merge(ryframe_api::generated::post::handler::router(
            service,
            PaginationPolicy::new(20, 200),
        ))
        .nest("/posts", export);
}

#[test]
fn message_response_preserves_utc_precision_null_and_schema() {
    use ryframe_api::message_presenter::MessageVo;
    use ryframe_application::ports::utc_timestamp::from_chrono;
    use serde_json::json;
    use utoipa::PartialSchema;

    for (nanoseconds, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        let historical = chrono::DateTime::from_timestamp(-1, nanoseconds).unwrap();
        let timestamp = from_chrono(historical).unwrap();
        assert_eq!(timestamp.offset(), time::UtcOffset::UTC);
        assert_eq!(timestamp.unix_timestamp(), -1);
        assert_eq!(timestamp.nanosecond(), nanoseconds);
        let expected = format!("1969-12-31T23:59:59{fraction}Z");
        let mut message = MessageVo {
            id: "42".into(),
            topic: "fixture".into(),
            title: "测试消息".into(),
            content: "消息正文".into(),
            severity: "info".into(),
            payload: None,
            published_at: timestamp,
            expires_at: None,
            acked_at: Some(timestamp),
            read_at: None,
        };
        let body = serde_json::to_value(&message).unwrap();
        assert_eq!(body["published_at"], expected);
        assert_eq!(body["acked_at"], expected);
        assert!(body["expires_at"].is_null());
        assert!(body["read_at"].is_null());
        message.expires_at = Some(timestamp);
        message.acked_at = None;
        message.read_at = Some(timestamp);
        let body = serde_json::to_value(message).unwrap();
        assert_eq!(body["expires_at"], expected);
        assert_eq!(body["read_at"], expected);
        assert!(body["acked_at"].is_null());
    }

    let schema = serde_json::to_value(MessageVo::schema()).unwrap();
    for name in ["published_at", "expires_at", "acked_at", "read_at"] {
        let property = &schema["properties"][name];
        assert_eq!(property["format"], "date-time");
        assert_eq!(
            property["type"],
            if name == "published_at" {
                json!("string")
            } else {
                json!(["string", "null"])
            }
        );
    }
    let leap = chrono::DateTime::from_timestamp(1_483_228_799, 1_123_456_000).unwrap();
    assert!(from_chrono(leap).is_err());
    let out_of_range = chrono::DateTime::from_timestamp(253_402_300_800, 0).unwrap();
    assert!(from_chrono(out_of_range).is_err());
}

#[test]
fn data_retention_overview_and_preview_preserve_utc_json_and_schema() {
    use std::collections::BTreeMap;

    use ryframe_api::dto::public_dto::{DataRetentionOverview, DataRetentionPreview};
    use ryframe_application::system::operations::{
        DataRetentionCutoffVo as ServiceCutoff, DataRetentionOverview as ServiceOverview,
        DataRetentionPolicy, DataRetentionPreview as ServicePreview,
    };
    use ryframe_kernel::AppError;
    use serde_json::to_value;
    use time::{Date, OffsetDateTime, macros::offset};

    let policy = DataRetentionPolicy {
        cleanup_batch_size: 100,
        max_rows_per_resource_per_run: 1_000,
        background_job_succeeded_days: 30,
        outbox_published_days: 30,
        schedule_execution_days: 30,
        export_job_history_days: 30,
        operation_log_days: 90,
        login_log_days: 90,
        user_import_history_days: 30,
        user_import_artifact_hours: 24,
        tenant_config_artifact_hours: 24,
        tenant_config_rollback_hours: 24,
        retention_run_days: 90,
        dead_background_jobs_permanent: true,
        dead_outbox_events_permanent: true,
    };
    let cutoff = |before| ServiceCutoff {
        resource: "background_jobs".into(),
        before,
    };
    for (nanos, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        let calculated_at = OffsetDateTime::from_unix_timestamp_nanos(-1_000_000_000 + nanos)
            .unwrap()
            .to_offset(offset!(+05:30));
        let before = OffsetDateTime::from_unix_timestamp_nanos(-2_000_000_000 + nanos).unwrap();
        let overview = ServiceOverview {
            calculated_at,
            policy,
            cutoffs: vec![cutoff(before)],
        };
        let preview = ServicePreview {
            calculated_at,
            policy,
            cutoffs: vec![cutoff(before)],
            eligible_counts: BTreeMap::from([("background_jobs".into(), 3)]),
        };
        let old_overview = to_value(&overview).unwrap();
        let old_preview = to_value(&preview).unwrap();
        let overview = to_value(DataRetentionOverview::try_from(overview).unwrap()).unwrap();
        let preview = to_value(DataRetentionPreview::try_from(preview).unwrap()).unwrap();
        assert_eq!(overview, old_overview);
        assert_eq!(preview, old_preview);
        for response in [overview, preview] {
            assert_eq!(
                response["calculated_at"],
                format!("1969-12-31T23:59:59{fraction}Z")
            );
            assert_eq!(
                response["cutoffs"][0]["before"],
                format!("1969-12-31T23:59:58{fraction}Z")
            );
        }
    }

    assert_retention_time_schema();

    let outside_utc = Date::MIN.midnight().assume_offset(offset!(+01:00));
    let overview = ServiceOverview {
        calculated_at: outside_utc,
        policy,
        cutoffs: Vec::new(),
    };
    assert!(matches!(
        DataRetentionOverview::try_from(overview),
        Err(AppError::Internal(message)) if message.contains("calculated_at")
    ));
    let preview = ServicePreview {
        calculated_at: OffsetDateTime::UNIX_EPOCH,
        policy,
        cutoffs: vec![cutoff(outside_utc)],
        eligible_counts: BTreeMap::new(),
    };
    assert!(matches!(
        DataRetentionPreview::try_from(preview),
        Err(AppError::Internal(message)) if message.contains("before")
    ));
}

#[test]
fn tenant_backup_points_preserve_utc_precision_nullability_and_schema() {
    use chrono::DateTime;
    use ryframe_api::dto::tenant_data_dto::BackupPointView;
    use ryframe_application::{
        ports::tenant_data::TenantDataBackupPointRecord,
        system::platform::BackupPointView as ServiceBackupPoint,
    };
    use ryframe_kernel::AppError;
    use serde_json::{Value, from_str, json, to_string, to_value};

    let epoch = DateTime::from_timestamp(0, 0).unwrap();
    let mut record = TenantDataBackupPointRecord {
        id: 42,
        scope: "tenant".into(),
        tenant_id: Some("tenant-a".into()),
        target_key: "target-a".into(),
        placement_generation: Some(9),
        schema_fingerprint: "schema-a".into(),
        provider_ref: "private-provider-reference".into(),
        captured_at: epoch,
        checksum: Some("checksum-a".into()),
        validation_status: "valid".into(),
        validation_detail: Some("private-validation-detail".into()),
        retention_until: epoch,
        expires_at: None,
        last_restore_drill_at: None,
        created_by: Some(7),
        created_at: epoch,
        updated_at: epoch,
    };
    for (nanos, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        record.captured_at = DateTime::from_timestamp(-1, nanos).unwrap();
        record.retention_until = DateTime::from_timestamp(2, nanos).unwrap();
        record.expires_at = DateTime::from_timestamp(3, nanos);
        record.last_restore_drill_at = DateTime::from_timestamp(0, nanos);
        let old = json!({
            "id": "42",
            "scope": "tenant",
            "tenant_id": "tenant-a",
            "target_key": "target-a",
            "placement_generation": "9",
            "schema_fingerprint": "schema-a",
            "captured_at": record.captured_at,
            "checksum": "checksum-a",
            "validation_status": "valid",
            "retention_until": record.retention_until,
            "expires_at": record.expires_at,
            "last_restore_drill_at": record.last_restore_drill_at,
        });
        assert_eq!(
            old["captured_at"],
            format!("1969-12-31T23:59:59{fraction}Z")
        );
        let service = ServiceBackupPoint::try_from(record.clone()).unwrap();
        for payload in [
            to_string(&service).unwrap(),
            to_string(&BackupPointView::try_from(service).unwrap()).unwrap(),
        ] {
            let response: Value = from_str(&payload).unwrap();
            assert_eq!(response, old);
            assert!(response.get("provider_ref").is_none());
            assert!(response.get("validation_detail").is_none());
        }
    }

    record.expires_at = None;
    record.last_restore_drill_at = None;
    let service = ServiceBackupPoint::try_from(record.clone()).unwrap();
    for response in [
        to_value(&service).unwrap(),
        to_value(BackupPointView::try_from(service).unwrap()).unwrap(),
    ] {
        assert_eq!(response.get("expires_at"), Some(&Value::Null));
        assert_eq!(response.get("last_restore_drill_at"), Some(&Value::Null));
    }

    assert_backup_point_time_schema();

    record.last_restore_drill_at = DateTime::from_timestamp(-1, 1_123_456_000);
    assert!(matches!(
        ServiceBackupPoint::try_from(record),
        Err(AppError::Internal(message)) if message.contains("last_restore_drill_at")
    ));
}

fn assert_retention_time_schema() {
    use ryframe_api::dto::public_dto::{
        DataRetentionCutoff, DataRetentionOverview, DataRetentionPreview,
    };
    use serde_json::to_value;
    use utoipa::PartialSchema;

    assert_required_date_time(&to_value(DataRetentionCutoff::schema()).unwrap(), "before");
    assert_required_date_time(
        &to_value(DataRetentionOverview::schema()).unwrap(),
        "calculated_at",
    );
    assert_required_date_time(
        &to_value(DataRetentionPreview::schema()).unwrap(),
        "calculated_at",
    );
}

fn assert_backup_point_time_schema() {
    use ryframe_api::dto::tenant_data_dto::BackupPointView;
    use serde_json::{json, to_value};
    use utoipa::PartialSchema;

    let schema = to_value(BackupPointView::schema()).unwrap();
    for field in ["captured_at", "retention_until"] {
        assert_required_date_time(&schema, field);
    }
    let required = schema["required"].as_array().unwrap();
    for field in ["expires_at", "last_restore_drill_at"] {
        assert_eq!(
            schema["properties"][field],
            json!({"type":["string","null"],"format":"date-time"})
        );
        assert!(!required.contains(&json!(field)));
    }
    for field in ["provider_ref", "validation_detail", "created_by"] {
        assert!(schema["properties"].get(field).is_none());
    }
}

fn assert_required_date_time(schema: &serde_json::Value, field: &str) {
    use serde_json::json;

    assert_eq!(
        schema["properties"][field],
        json!({"type":"string","format":"date-time"})
    );
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!(field))
    );
}

#[test]
fn background_job_response_preserves_utc_precision_nullability_and_schema() {
    use ryframe_api::dto::public_dto::BackgroundJobVo;
    use ryframe_application::BackgroundJobVo as ServiceJob;
    use ryframe_kernel::AppError;
    use serde_json::{Value, json};
    use time::{
        Date, OffsetDateTime,
        macros::{datetime, offset},
    };
    use utoipa::PartialSchema;

    let epoch = OffsetDateTime::UNIX_EPOCH;
    let mut job = ServiceJob {
        id: "42".into(),
        schedule_id: Some("9".into()),
        scheduled_for: None,
        max_runtime_seconds: Some(60),
        job_type: "fixture.job".into(),
        status: "pending".into(),
        priority: 10,
        available_at: epoch,
        attempts: 0,
        max_attempts: 3,
        lease_owner: Some("worker-fixture".into()),
        lease_until: None,
        dedupe_key: Some("dedupe-fixture".into()),
        last_error: Some("已脱敏".into()),
        created_at: epoch,
        updated_at: epoch,
        completed_at: None,
    };

    for (nanos, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        let at = |seconds: i64| {
            OffsetDateTime::from_unix_timestamp(seconds)
                .unwrap()
                .replace_nanosecond(nanos)
                .unwrap()
        };
        job.scheduled_for = Some(at(-2));
        job.available_at = at(-1);
        job.lease_until = Some(at(4));
        job.created_at = at(-3);
        job.updated_at = at(1);
        job.completed_at = Some(at(2));
        let expected = serde_json::to_value(&job).unwrap();
        let encoded =
            serde_json::to_string(&BackgroundJobVo::try_from(job.clone()).unwrap()).unwrap();
        let response: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(response, expected);
        for (field, base) in [
            ("scheduled_for", "1969-12-31T23:59:58"),
            ("available_at", "1969-12-31T23:59:59"),
            ("lease_until", "1970-01-01T00:00:04"),
            ("created_at", "1969-12-31T23:59:57"),
            ("updated_at", "1970-01-01T00:00:01"),
            ("completed_at", "1970-01-01T00:00:02"),
        ] {
            assert_eq!(response[field], format!("{base}{fraction}Z"));
        }
        assert!(response.get("payload").is_none());
    }

    job.scheduled_for = None;
    job.lease_until = None;
    job.completed_at = None;
    job.created_at = datetime!(1000-01-01 0:00 UTC);
    job.updated_at = datetime!(9999-12-31 23:59:59.499999 UTC);
    let response = serde_json::to_value(BackgroundJobVo::try_from(job.clone()).unwrap()).unwrap();
    assert_eq!(response, serde_json::to_value(&job).unwrap());
    for field in ["scheduled_for", "lease_until", "completed_at"] {
        assert_eq!(response.get(field), Some(&Value::Null));
    }

    let schema = serde_json::to_value(BackgroundJobVo::schema()).unwrap();
    let required = schema["required"].as_array().unwrap();
    for field in ["available_at", "created_at", "updated_at"] {
        assert_eq!(
            schema["properties"][field],
            json!({"type":"string","format":"date-time"})
        );
        assert!(required.contains(&json!(field)));
    }
    for field in ["scheduled_for", "lease_until", "completed_at"] {
        assert_eq!(
            schema["properties"][field],
            json!({"type":["string","null"],"format":"date-time"})
        );
        assert!(!required.contains(&json!(field)));
    }
    assert!(schema["properties"].get("payload").is_none());

    job.lease_until = Some(
        Date::MAX
            .with_hms(23, 59, 59)
            .unwrap()
            .assume_offset(offset!(-01:00)),
    );
    assert!(matches!(
        BackgroundJobVo::try_from(job),
        Err(AppError::Internal(message)) if message.contains("lease_until")
    ));
}

#[test]
fn export_job_response_preserves_utc_precision_nullability_and_schema() {
    use chrono::{DateTime, Utc};
    use ryframe_api::dto::public_dto::ExportJobVo;
    use ryframe_application::ports::export::ExportRequesterRecord;
    use ryframe_application::system::operations::export_requester_view;
    use ryframe_kernel::AppError;
    use serde_json::{Value, json};
    use utoipa::OpenApi;

    let job = export_requester_view(ExportRequesterRecord {
        id: 42,
        resource: "users".into(),
        status: "succeeded".into(),
        result_file_name: Some("users-42.xlsx".into()),
        content_type: Some(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
        ),
        file_size: Some(256),
        expires_at: DateTime::from_timestamp(20, 0),
        error_message: None,
        snapshot_at: DateTime::from_timestamp(0, 123_400_000).unwrap(),
        matched_rows: 8,
        created_at: DateTime::from_timestamp(0, 120_000_000).unwrap(),
        updated_at: DateTime::from_timestamp(10, 0).unwrap(),
        completed_at: DateTime::from_timestamp(9, 123_456_700),
        notification_read_at: DateTime::from_timestamp(10, 0),
        permission_code: "system:user:export".into(),
        request_params: json!({"selected_ids": [10]}),
        request_version: 1,
        authorization_fingerprint: "authorization-fixture".into(),
        upper_id: 99,
        result_file_id: Some(7),
    });
    let encoded = serde_json::to_string(&ExportJobVo::try_from(job.clone()).unwrap()).unwrap();
    assert_eq!(encoded, serde_json::to_string(&job).unwrap());
    let response: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(response["id"], "42");
    assert_eq!(response["result_file_name"], "users-42.xlsx");
    assert_eq!(response["snapshot_at"], "1970-01-01T00:00:00.123400Z");
    assert_eq!(response["created_at"], "1970-01-01T00:00:00.120Z");
    assert_eq!(response["updated_at"], "1970-01-01T00:00:10Z");
    assert_eq!(response["expires_at"], "1970-01-01T00:00:20Z");
    assert_eq!(response["completed_at"], "1970-01-01T00:00:09.123456700Z");
    assert_eq!(response["notification_read_at"], "1970-01-01T00:00:10Z");

    let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let schema = &document["components"]["schemas"]["ExportJobVo"];
    let required = schema["required"].as_array().unwrap();
    for field in ["snapshot_at", "created_at", "updated_at"] {
        assert_eq!(
            schema["properties"][field],
            json!({"type":"string","format":"date-time"})
        );
        assert!(required.contains(&json!(field)));
    }
    for field in [
        "permission_code",
        "request_params",
        "request_version",
        "authorization_fingerprint",
        "upper_id",
        "result_file_id",
    ] {
        assert!(response.get(field).is_none());
        assert!(schema["properties"].get(field).is_none());
    }

    let mut job = job;
    job.expires_at = None;
    job.completed_at = None;
    job.notification_read_at = None;
    job.created_at = DateTime::parse_from_rfc3339("1000-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    job.snapshot_at = DateTime::parse_from_rfc3339("9999-12-31T23:59:59.499999Z")
        .unwrap()
        .with_timezone(&Utc);
    let response = serde_json::to_value(ExportJobVo::try_from(job.clone()).unwrap()).unwrap();
    assert_eq!(response, serde_json::to_value(&job).unwrap());
    assert_eq!(response["created_at"], "1000-01-01T00:00:00Z");
    assert_eq!(response["snapshot_at"], "9999-12-31T23:59:59.499999Z");
    for field in ["expires_at", "completed_at", "notification_read_at"] {
        assert_eq!(response.get(field), Some(&Value::Null));
        assert_eq!(
            schema["properties"][field],
            json!({"type":["string","null"],"format":"date-time"})
        );
        assert!(!required.contains(&json!(field)));
    }

    job.snapshot_at = DateTime::from_timestamp(-1, 1_123_456_000).unwrap();
    assert!(matches!(
        ExportJobVo::try_from(job),
        Err(AppError::Internal(message)) if message.contains("snapshot_at")
    ));
}

#[test]
fn schedule_execution_response_preserves_utc_precision_and_schema() {
    use ryframe_api::dto::public_dto::JobScheduleExecutionVo;
    use ryframe_application::{
        JobScheduleExecutionVo as ServiceExecution, ports::jobs::JobScheduleExecutionRecord,
    };
    use serde_json::{Value, json};
    use time::macros::datetime;
    use utoipa::OpenApi;

    let mut execution = ServiceExecution::from(JobScheduleExecutionRecord {
        id: 42,
        tenant_id: "tenant-fixture".into(),
        schedule_id: 9,
        schedule_name: "调度样例".into(),
        handler_key: "fixture.job".into(),
        fire_key: "manual:fixture".into(),
        trigger_kind: "manual".into(),
        scheduled_for: datetime!(1969-12-31 23:59:59.123400 UTC),
        outcome: "enqueued".into(),
        background_job_id: Some(7),
        background_job_status: Some("pending".into()),
        detail: None,
        created_at: datetime!(1970-01-01 00:00:00.120 UTC),
    });
    let encoded = serde_json::to_string(&JobScheduleExecutionVo::from(execution.clone())).unwrap();
    assert_eq!(encoded, serde_json::to_string(&execution).unwrap());
    let response: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(response["id"], "42");
    assert_eq!(response["schedule_id"], "9");
    assert_eq!(response["background_job_id"], "7");
    assert_eq!(response["scheduled_for"], "1969-12-31T23:59:59.123400Z");
    assert_eq!(response["created_at"], "1970-01-01T00:00:00.120Z");
    assert_eq!(response.get("detail"), Some(&Value::Null));
    assert!(response.get("tenant_id").is_none());
    assert!(response.get("fire_key").is_none());

    let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
    assert_eq!(
        document["components"]["schemas"]["JobScheduleExecutionVo"],
        json!({
            "type": "object",
            "required": [
                "id", "schedule_id", "schedule_name", "handler_key",
                "trigger_kind", "scheduled_for", "outcome", "created_at"
            ],
            "properties": {
                "id": {"type": "string"},
                "schedule_id": {"type": "string"},
                "schedule_name": {"type": "string"},
                "handler_key": {"type": "string"},
                "trigger_kind": {"type": "string"},
                "scheduled_for": {"type": "string", "format": "date-time"},
                "outcome": {"type": "string"},
                "background_job_id": {"type": ["string", "null"]},
                "background_job_status": {"type": ["string", "null"]},
                "detail": {"type": ["string", "null"]},
                "created_at": {"type": "string", "format": "date-time"}
            }
        })
    );

    execution.scheduled_for = datetime!(1000-01-01 00:00:00 UTC);
    execution.created_at = datetime!(9999-12-31 23:59:59.499999 UTC);
    let response = serde_json::to_value(JobScheduleExecutionVo::from(execution.clone())).unwrap();
    assert_eq!(response, serde_json::to_value(&execution).unwrap());
    assert_eq!(response["scheduled_for"], "1000-01-01T00:00:00Z");
    assert_eq!(response["created_at"], "9999-12-31T23:59:59.499999Z");
}

#[test]
fn product_plan_version_response_preserves_published_at_utc_and_schema() {
    use ryframe_api::dto::product_dto::ProductPlanVersionVo;
    use ryframe_application::system::platform::ProductPlanVersionVo as ServiceVersion;
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    let response = |published_at: Option<OffsetDateTime>| {
        let service_version = ServiceVersion {
            id: "42".into(),
            version: 1,
            name: "标准版".into(),
            description: None,
            status: if published_at.is_some() {
                "published"
            } else {
                "draft"
            }
            .into(),
            created_by: "1".into(),
            published_by: published_at.map(|_| "1".into()),
            published_at,
            capabilities: Vec::new(),
        };
        let service_json = serde_json::to_value(&service_version).unwrap();
        let version = ProductPlanVersionVo::try_from(service_version).unwrap();
        let response_json = serde_json::to_value(version).unwrap();
        assert_eq!(response_json["published_at"], service_json["published_at"]);
        response_json
    };
    assert!(response(None)["published_at"].is_null());
    let non_utc = UtcOffset::from_hms(8, 0, 0).unwrap();
    for (nanos, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        let instant = OffsetDateTime::UNIX_EPOCH
            .replace_nanosecond(nanos)
            .unwrap()
            .to_offset(non_utc);
        assert_eq!(
            response(Some(instant))["published_at"],
            format!("1970-01-01T00:00:00{fraction}Z")
        );
    }
    let schema = serde_json::to_value(ProductPlanVersionVo::schema()).unwrap();
    let property = &schema["properties"]["published_at"];
    assert_eq!(property["format"], "date-time");
    assert_eq!(property["type"], json!(["string", "null"]));
}

#[test]
fn user_response_preserves_created_at_utc_precision_and_required_schema() {
    use ryframe_api::dto::public_dto::UserVo;
    use ryframe_application::system::identity::UserVo as ServiceUser;
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    let response = |created_at| {
        let service_user = ServiceUser {
            id: "42".into(),
            username: "tester".into(),
            nickname: "测试用户".into(),
            email: String::new(),
            phone: String::new(),
            avatar: None,
            status: "1".into(),
            dept_id: None,
            dept_name: None,
            remark: None,
            created_at,
        };
        let service_json = serde_json::to_value(&service_user).unwrap();
        let response_json = serde_json::to_value(UserVo::from(service_user)).unwrap();
        assert_eq!(response_json["created_at"], service_json["created_at"]);
        response_json
    };
    let non_utc = UtcOffset::from_hms(8, 0, 0).unwrap();
    for (nanos, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        let instant = OffsetDateTime::from_unix_timestamp(-1)
            .unwrap()
            .replace_nanosecond(nanos)
            .unwrap()
            .to_offset(non_utc);
        assert_eq!(
            response(instant)["created_at"],
            format!("1969-12-31T23:59:59{fraction}Z")
        );
    }
    let schema = serde_json::to_value(UserVo::schema()).unwrap();
    let property = &schema["properties"]["created_at"];
    assert_eq!(property["format"], "date-time");
    assert_eq!(property["type"], json!("string"));
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("created_at"))
    );
}

mod permission_catalog {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn catalog_contains_governance_permission_and_unique_sets() {
        assert!(permission_codes().contains(&"tenant:capability:override"));
        assert_eq!(
            permission_codes().len(),
            permission_codes()
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
        );
        assert_eq!(
            menu_routes().len(),
            menu_routes()
                .iter()
                .map(|menu| menu.route_key)
                .collect::<BTreeSet<_>>()
                .len()
        );
        assert_eq!(
            route_policies().len(),
            route_policies()
                .iter()
                .map(|route| (route.method, route.path))
                .collect::<BTreeSet<_>>()
                .len()
        );
    }

    #[test]
    fn generated_post_access_is_merged_once_with_product_capability() {
        for permission in [
            "system:post:add",
            "system:post:edit",
            "system:post:export",
            "system:post:list",
            "system:post:remove",
        ] {
            assert_eq!(
                permission_codes()
                    .iter()
                    .filter(|candidate| **candidate == permission)
                    .count(),
                1,
                "生成权限必须且只能合并一次: {permission}"
            );
        }

        let menus = menu_routes()
            .iter()
            .filter(|menu| menu.route_key == "system.post")
            .collect::<Vec<_>>();
        assert_eq!(menus.len(), 1, "生成岗位菜单必须且只能合并一次");
        assert_eq!(menus[0].name, "岗位管理");
        assert_eq!(menus[0].page_key, Some("system.post"));
        assert_eq!(menus[0].permission_code, Some("system:post:list"));
        assert_eq!(menus[0].capability_code, None);

        let post_routes = route_policies()
            .iter()
            .filter(|route| route.path.starts_with("/api/v1/system/posts"))
            .map(|route| (route.method, route.path, route.permission_code))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            post_routes,
            BTreeSet::from([
                (
                    "DELETE",
                    "/api/v1/system/posts/{id}",
                    Some("system:post:remove"),
                ),
                ("GET", "/api/v1/system/posts", Some("system:post:list")),
                ("GET", "/api/v1/system/posts/{id}", Some("system:post:list"),),
                ("POST", "/api/v1/system/posts", Some("system:post:add")),
                (
                    "POST",
                    "/api/v1/system/posts/exports",
                    Some("system:post:export"),
                ),
                ("PUT", "/api/v1/system/posts/{id}", Some("system:post:edit"),),
            ])
        );
    }

    #[test]
    fn route_policy_constraints_match_policy_kind() {
        assert!(SUPPORTED_HTTP_METHODS.contains(&"PATCH"));
        for route in route_policies() {
            assert!(SUPPORTED_HTTP_METHODS.contains(&route.method));
            match route.policy {
                AccessPolicy::Public | AccessPolicy::Authenticated => {
                    assert_eq!(route.permission_code, None);
                    assert_eq!(route.capability_code, None);
                }
                AccessPolicy::Permission => {
                    assert!(route.permission_code.is_some());
                    assert_eq!(route.capability_code, None);
                }
                AccessPolicy::Capability => assert!(route.capability_code.is_some()),
            }
        }
    }

    #[test]
    fn menu_references_are_closed() {
        let permissions = permission_codes().iter().copied().collect::<BTreeSet<_>>();
        let capabilities = capabilities()
            .iter()
            .map(|capability| capability.code)
            .collect::<BTreeSet<_>>();
        for menu in menu_routes() {
            if let Some(permission) = menu.permission_code {
                assert!(permissions.contains(permission));
            }
            if let Some(capability) = menu.capability_code {
                assert!(capabilities.contains(capability));
            }
        }
    }

    #[test]
    fn capability_catalog_matches_application_contract() {
        assert_eq!(
            capabilities().len(),
            ryframe_application::system::platform::CAPABILITY_CATALOG.len()
        );
        for capability in capabilities() {
            let application = ryframe_application::system::platform::CAPABILITY_CATALOG
                .iter()
                .find(|candidate| candidate.code == capability.code)
                .expect("访问目录能力必须存在于应用能力目录");
            assert_eq!(capability.route_keys, application.route_keys);
            assert_eq!(capability.permission_codes, application.permission_codes);
        }
    }
}

mod openapi {
    use std::collections::BTreeSet;

    use serde_json::Value;
    use utoipa::OpenApi;

    use super::ApiDoc;

    #[test]
    fn menu_route_extension_exactly_matches_compiled_catalog() {
        let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI 必须可序列化");
        let routes = document["x-ryframe-menu-routes"]["routes"]
            .as_array()
            .expect("OpenAPI 必须输出菜单路由数组");
        let catalog = ryframe_api::permission_catalog::menu_routes();
        assert_eq!(routes.len(), catalog.len());

        let mut page_keys = BTreeSet::new();
        for (route, menu) in routes.iter().zip(catalog) {
            assert_eq!(
                route,
                &serde_json::json!({
                    "route_key": menu.route_key,
                    "name": menu.name,
                    "title_key": menu.title_key,
                    "menu_type": menu.menu_type,
                    "icon": menu.icon,
                    "page_key": menu.page_key,
                    "permission_code": menu.permission_code,
                    "capability_code": menu.capability_code,
                })
            );
            match menu.menu_type {
                "M" => assert!(route["page_key"].is_null()),
                "C" => {
                    let page_key = route["page_key"]
                        .as_str()
                        .filter(|page_key| !page_key.is_empty())
                        .expect("页面菜单必须输出非空 page_key");
                    assert!(page_keys.insert(page_key), "页面 page_key 必须唯一");
                }
                menu_type => panic!("访问目录存在未知菜单类型: {menu_type}"),
            }
        }
    }

    #[test]
    fn role_option_purpose_is_required_and_complete_in_openapi() {
        let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI 必须可序列化");
        let parameters = document["paths"]["/api/v1/system/roles/options"]["get"]["parameters"]
            .as_array()
            .expect("角色选项接口必须声明查询参数");
        let purpose = parameters
            .iter()
            .find(|parameter| parameter["name"] == "purpose")
            .expect("角色选项接口必须声明 purpose");
        assert_eq!(purpose["in"], "query");
        assert_eq!(purpose["required"], true);
        assert_eq!(
            purpose["schema"]["$ref"],
            "#/components/schemas/RoleOptionPurposeDto"
        );

        let values = document["components"]["schemas"]["RoleOptionPurposeDto"]["enum"]
            .as_array()
            .expect("角色选项用途必须声明枚举")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(values, ["user_assignment"]);
    }

    #[test]
    fn user_detail_exposes_an_optional_department_without_a_new_endpoint() {
        let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI 必须可序列化");
        let operation = &document["paths"]["/api/v1/system/users/{id}"]["get"];
        assert_eq!(operation["operationId"], "get_system_users_by_id");

        let department = document["components"]["schemas"]["UserDetailVo"]["allOf"][1]
            ["properties"]["department"]["oneOf"]
            .as_array()
            .expect("用户详情必须声明可空部门对象");
        assert!(department.iter().any(|schema| schema["type"] == "null"));
        assert!(
            department
                .iter()
                .any(|schema| schema["$ref"] == "#/components/schemas/DeptVo")
        );
    }

    #[test]
    fn generated_crud_and_manual_extensions_share_one_openapi_document() {
        let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI 必须可序列化");
        let paths = document["paths"]
            .as_object()
            .expect("OpenAPI 必须包含 paths");

        assert_eq!(
            paths["/api/v1/system/posts"]["get"]["operationId"],
            "get_system_posts"
        );
        assert_eq!(
            paths["/api/v1/system/posts"]["post"]["operationId"],
            "post_system_posts"
        );
        assert_eq!(
            paths["/api/v1/system/posts/{id}"]["get"]["operationId"],
            "get_system_posts_by_id"
        );
        assert_eq!(
            paths["/api/v1/system/posts/{id}"]["put"]["operationId"],
            "put_system_posts_by_id"
        );
        assert_eq!(
            paths["/api/v1/system/posts/{id}"]["delete"]["operationId"],
            "delete_system_posts_by_id"
        );
        assert_eq!(
            paths["/api/v1/system/posts/exports"]["post"]["operationId"],
            "post_system_posts_exports"
        );
        assert_eq!(
            paths["/api/v1/system/notices"]["get"]["operationId"],
            "get_system_notices"
        );
        assert_eq!(
            paths["/api/v1/system/notices/{id}/publish-message"]["post"]["operationId"],
            "post_system_notices_by_id_publish_message"
        );

        let resources = document["x-ryframe-crud-resources"]["resources"]
            .as_array()
            .expect("OpenAPI 必须保留生成 CRUD 资源扩展");
        let names = resources
            .iter()
            .map(|resource| resource["name"].as_str().expect("资源名必须是字符串"))
            .collect::<Vec<_>>();
        assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
        let names = names.into_iter().collect::<BTreeSet<_>>();
        assert!(names.contains("notice"));
        assert!(names.contains("post"));
    }
}
