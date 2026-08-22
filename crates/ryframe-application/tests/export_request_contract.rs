use chrono::{DateTime, Utc};
use ryframe_application::{ports::export::*, system::export::*};
use ryframe_kernel::*;

mod filters {
    use super::*;

    #[test]
    fn normalizes_text_and_preserves_numeric_zero() {
        let selection = ExportSelection::Users(UserExportFilter::new(
            Some("  alice  ".into()),
            Some("   ".into()),
            Some("0".into()),
            Some(0),
        ));
        let ExportSelection::Users(filter) = selection else {
            panic!("应为用户筛选");
        };
        assert_eq!(filter.username(), Some("alice"));
        assert_eq!(filter.phone(), None);
        assert_eq!(filter.status(), Some("0"));
        assert_eq!(filter.dept_id(), Some(0));
        assert!(!filter.is_empty());
    }

    #[test]
    fn requires_confirmation_for_empty_filter() {
        let command = RequestExportCommand {
            permission_code: "system:role:export".into(),
            selection: ExportSelection::Roles(RoleExportFilter::new(None, None, None)),
            confirm_all: false,
        };
        let error = validate_request_command(&command).expect_err("空筛选必须拒绝");
        assert_eq!(
            error.error_code().as_str(),
            "EXPORT_ALL_CONFIRMATION_REQUIRED"
        );

        let confirmed = RequestExportCommand {
            confirm_all: true,
            ..command
        };
        validate_request_command(&confirmed).expect("显式确认后应允许空筛选");
    }

    #[test]
    fn requires_rfc3339_timezone_and_normalizes_to_utc() {
        let filter = OperLogExportFilter::new(
            Some(" operator ".into()),
            None,
            Some("2026-08-20T10:00:00+08:00".into()),
            Some("2026-08-20T03:00:00Z".into()),
        )
        .expect("有效时间区间应通过");
        assert_eq!(filter.oper_name(), Some("operator"));
        assert_eq!(
            filter.begin_time().map(|time| time.to_rfc3339()),
            Some("2026-08-20T02:00:00+00:00".into())
        );

        let missing_timezone =
            LoginLogExportFilter::new(None, None, Some("2026-08-20T10:00:00".into()), None);
        assert!(matches!(missing_timezone, Err(AppError::Validation(_))));

        let reversed = LoginLogExportFilter::new(
            None,
            None,
            Some("2026-08-20T04:00:00Z".into()),
            Some("2026-08-20T03:00:00Z".into()),
        );
        assert!(matches!(reversed, Err(AppError::Validation(_))));
    }

    #[test]
    fn persisted_filter_rejects_pagination_and_unknown_fields() {
        let with_page = serde_json::json!({
            "resource": "roles",
            "filter": {"name": "ops", "page": 2}
        });
        assert!(serde_json::from_value::<ExportSelection>(with_page).is_err());

        let unknown_resource_field = serde_json::json!({
            "resource": "roles",
            "filter": {"name": "ops"},
            "legacy": true
        });
        assert!(serde_json::from_value::<ExportSelection>(unknown_resource_field).is_err());
    }

    #[test]
    fn authorization_change_fails_closed() {
        ensure_download_authorization_matches("fingerprint-a", "fingerprint-a")
            .expect("相同授权指纹应通过");
        assert!(matches!(
            ensure_download_authorization_matches("fingerprint-a", "fingerprint-b"),
            Err(AppError::Authorization(_))
        ));
        assert!(matches!(
            ensure_download_authorization_matches("", "fingerprint-b"),
            Err(AppError::Authorization(_))
        ));
    }

    #[test]
    fn only_committed_success_keeps_the_uploaded_object() {
        assert!(!should_delete_uncommitted_object(EXPORT_STATUS_SUCCEEDED));
        assert!(should_delete_uncommitted_object("running"));
        assert!(should_delete_uncommitted_object("failed"));
    }
}

mod lifecycle {
    use super::*;

    fn requester_record() -> ExportRequesterRecord {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-21T00:00:00Z")
            .expect("测试时间应有效")
            .with_timezone(&chrono::Utc);
        ExportRequesterRecord {
            id: 42,
            resource: "users".into(),
            status: EXPORT_STATUS_SUCCEEDED.into(),
            result_file_name: Some("users-42.xlsx".into()),
            content_type: Some(
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".into(),
            ),
            file_size: Some(128),
            expires_at: Some(now),
            error_message: None,
            snapshot_at: now,
            matched_rows: 8,
            created_at: now,
            updated_at: now,
            completed_at: Some(now),
            notification_read_at: None,
            permission_code: "system:user:export".into(),
            request_params: serde_json::json!({"request_version": EXPORT_REQUEST_VERSION}),
            request_version: i32::from(EXPORT_REQUEST_VERSION),
            authorization_fingerprint: "authorization".into(),
            upper_id: 99,
            result_file_id: Some(7),
        }
    }

    #[test]
    fn requester_record_maps_without_database_types() {
        let record = requester_record();
        let snapshot = export_requester_snapshot(&record);
        assert_eq!(snapshot.request_version, i32::from(EXPORT_REQUEST_VERSION));
        assert_eq!(snapshot.authorization_fingerprint, "authorization");
        assert_eq!(snapshot.upper_id, 99);
        assert_eq!(snapshot.matched_rows, 8);

        let view = export_requester_view(record);
        assert_eq!(view.id, "42");
        assert_eq!(view.status, EXPORT_STATUS_SUCCEEDED);
        assert_eq!(view.result_file_name.as_deref(), Some("users-42.xlsx"));
    }

    #[test]
    fn deletion_ids_are_sorted_deduplicated_and_bounded() {
        let mut ids = vec![9, 3, 9, 5];
        normalize_deletion_ids(&mut ids).expect("有效 ID 应通过");
        assert_eq!(ids, vec![3, 5, 9]);

        assert!(normalize_deletion_ids(&mut Vec::new()).is_err());
        assert!(normalize_deletion_ids(&mut vec![0]).is_err());
        let mut too_many = (1..=101).collect::<Vec<_>>();
        assert!(normalize_deletion_ids(&mut too_many).is_err());

        assert_eq!(
            deletion_cleanup_dedupe_key("tenant-a", 7, &[3, 5, 9]),
            deletion_cleanup_dedupe_key("tenant-a", 7, &[3, 5, 9])
        );
        assert_ne!(
            deletion_cleanup_dedupe_key("tenant-a", 7, &[3, 5, 9]),
            deletion_cleanup_dedupe_key("tenant-a", 7, &[3, 5, 10])
        );
    }

    #[test]
    fn request_fingerprint_is_stable_and_authorization_sensitive() {
        let selection = ExportSelection::Roles(RoleExportFilter::new(
            Some(" ops ".into()),
            None,
            Some("0".into()),
        ));
        let first =
            calculate_request_fingerprint("tenant-a", 7, "system:role:export", &selection, "a")
                .expect("指纹应生成");
        let same =
            calculate_request_fingerprint("tenant-a", 7, "system:role:export", &selection, "a")
                .expect("同一输入应生成指纹");
        let changed =
            calculate_request_fingerprint("tenant-a", 7, "system:role:export", &selection, "b")
                .expect("变更授权仍应生成指纹");
        assert_eq!(first, same);
        assert_ne!(first, changed);
        let other_resource =
            ExportSelection::Configs(ConfigExportFilter::new(Some("ops".into()), None));
        for different in [
            calculate_request_fingerprint("tenant-b", 7, "system:role:export", &selection, "a"),
            calculate_request_fingerprint("tenant-a", 8, "system:role:export", &selection, "a"),
            calculate_request_fingerprint("tenant-a", 7, "system:other:export", &selection, "a"),
            calculate_request_fingerprint(
                "tenant-a",
                7,
                "system:role:export",
                &other_resource,
                "a",
            ),
        ] {
            assert_ne!(first, different.expect("不同输入仍应生成指纹"));
        }
        assert_eq!(first.len(), 64);
    }
}

mod preflight {
    use super::*;

    #[test]
    fn empty_selection_fails_without_creating_a_job() {
        let error = validate_export_summary(
            ExportQuerySnapshot {
                matched_rows: 0,
                upper_id: None,
            },
            500_000,
        )
        .expect_err("空结果必须同步失败");
        assert_eq!(error.error_code().as_str(), "EXPORT_NO_MATCHING_ROWS");
    }

    #[test]
    fn oversized_selection_preserves_count_and_limit() {
        let error = validate_export_summary(
            ExportQuerySnapshot {
                matched_rows: 500_001,
                upper_id: Some(900_001),
            },
            500_000,
        )
        .expect_err("超过上限必须同步失败");
        assert!(matches!(
            error,
            AppError::ExportRowLimitExceeded {
                matched_rows: 500_001,
                limit: 500_000,
            }
        ));
    }

    #[test]
    fn non_empty_selection_requires_a_positive_upper_id() {
        let error = validate_export_summary(
            ExportQuerySnapshot {
                matched_rows: 1,
                upper_id: None,
            },
            500_000,
        )
        .expect_err("非空结果必须具备主键上界");
        assert!(matches!(error, AppError::Database(_)));

        let snapshot = validate_export_summary(
            ExportQuerySnapshot {
                matched_rows: 8,
                upper_id: Some(99),
            },
            500_000,
        )
        .expect("合法选择应生成快照");
        assert_eq!(snapshot.matched_rows, 8);
        assert_eq!(snapshot.upper_id, 99);
    }
}

mod types {
    use super::*;
    use ryframe_application::system::RoleExportFilter;

    #[test]
    fn worker_accepts_only_current_strict_snapshot() {
        let valid = serde_json::json!({
            "request_version": EXPORT_REQUEST_VERSION,
            "selection": {
                "resource": "roles",
                "filter": {"name": "ops", "code": null, "status": "0"}
            },
            "authorization_fingerprint": "fingerprint-at-request",
            "snapshot_at": "2026-08-20T12:00:00Z",
            "upper_id": 88,
            "matched_rows": 12
        });
        let request: StoredExportRequest =
            serde_json::from_value(valid).expect("当前版本快照应可解析");
        request.validate("roles").expect("资源应匹配");

        let previous_version = serde_json::json!({
            "request_version": EXPORT_REQUEST_VERSION - 1,
            "selection": {
                "resource": "roles",
                "filter": {"name": "ops", "code": null, "status": "0"}
            },
            "authorization_fingerprint": "fingerprint-at-request",
            "snapshot_at": "2026-08-20T12:00:00Z",
            "upper_id": 88,
            "matched_rows": 12
        });
        let previous: StoredExportRequest =
            serde_json::from_value(previous_version).expect("旧版本结构仍可被类型读取");
        assert!(matches!(
            previous.validate("roles"),
            Err(ryframe_kernel::AppError::Validation(_))
        ));

        let old_shape = serde_json::json!({"request": {"name": "ops"}});
        assert!(serde_json::from_value::<StoredExportRequest>(old_shape).is_err());

        let unknown = serde_json::json!({
            "request_version": EXPORT_REQUEST_VERSION,
            "selection": {
                "resource": "roles",
                "filter": {"name": null, "code": null, "status": null}
            },
            "authorization_fingerprint": "fingerprint-at-request",
            "snapshot_at": "2026-08-20T12:00:00Z",
            "upper_id": 88,
            "matched_rows": 12,
            "legacy": true
        });
        assert!(serde_json::from_value::<StoredExportRequest>(unknown).is_err());
    }

    #[test]
    fn worker_rejects_version_or_resource_mismatch() {
        let request = StoredExportRequest {
            request_version: EXPORT_REQUEST_VERSION + 1,
            selection: ExportSelection::Roles(RoleExportFilter::new(None, None, None)),
            authorization_fingerprint: "fingerprint-at-request".into(),
            snapshot_at: DateTime::parse_from_rfc3339("2026-08-20T12:00:00Z")
                .expect("测试时间有效")
                .with_timezone(&Utc),
            upper_id: 88,
            matched_rows: 12,
        };
        assert!(matches!(
            request.validate("roles"),
            Err(ryframe_kernel::AppError::Validation(_))
        ));

        let current = StoredExportRequest {
            request_version: EXPORT_REQUEST_VERSION,
            selection: request.selection,
            authorization_fingerprint: request.authorization_fingerprint,
            snapshot_at: request.snapshot_at,
            upper_id: request.upper_id,
            matched_rows: request.matched_rows,
        };
        assert!(matches!(
            current.validate("users"),
            Err(ryframe_kernel::AppError::Validation(_))
        ));
    }

    #[test]
    fn job_payload_rejects_old_version_and_unknown_fields() {
        let valid: ExportJobPayload = serde_json::from_value(serde_json::json!({
            "resource": "users",
            "request_version": EXPORT_REQUEST_VERSION
        }))
        .expect("当前载荷应可解析");
        valid.validate().expect("当前载荷应可校验");

        let unknown = serde_json::json!({
            "resource": "users",
            "request_version": EXPORT_REQUEST_VERSION,
            "legacy": true
        });
        assert!(serde_json::from_value::<ExportJobPayload>(unknown).is_err());

        let old: ExportJobPayload = serde_json::from_value(serde_json::json!({
            "resource": "users",
            "request_version": 0
        }))
        .expect("旧版本载荷结构应可解析");
        assert!(matches!(
            old.validate(),
            Err(ryframe_kernel::AppError::Validation(_))
        ));
    }
}
