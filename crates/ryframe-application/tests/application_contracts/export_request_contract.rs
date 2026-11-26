use chrono::{DateTime, Utc};
use ryframe_application::{ports::export::*, system::operations::*};
use ryframe_kernel::*;

mod filters {
    use super::*;
    use sha2::{Digest, Sha256};
    use time::macros::datetime;

    #[test]
    fn selected_rows_are_normalized_persisted_and_need_no_all_confirmation() {
        let selection = ExportSelection::Roles(RoleExportFilter::new(None, None, None))
            .with_selected_ids(vec![9007199254740993, 2, 2])
            .expect("合法 ID");
        assert_eq!(selection.selected_ids(), &[2, 9007199254740993]);
        let persisted = serde_json::to_value(&selection).expect("可保存");
        let restored: ExportSelection = serde_json::from_value(persisted).expect("可恢复");
        assert_eq!(restored.selected_ids(), selection.selected_ids());
        validate_request_command(&RequestExportCommand {
            permission_code: "system:role:export".into(),
            selection,
            confirm_all: false,
        })
        .expect("勾选导出不需要全量确认");
        for ids in [vec![0], vec![-1], vec![1; 1001]] {
            assert!(
                ExportSelection::Roles(RoleExportFilter::new(None, None, None))
                    .with_selected_ids(ids)
                    .is_err()
            );
        }
        assert!(
            ExportSelection::Roles(RoleExportFilter::new(None, None, None))
                .with_selected_ids(vec![])
                .expect("空列表等于未勾选")
                .is_empty()
        );
    }

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
            filter.begin_time(),
            Some(datetime!(2026-08-20 02:00:00 UTC))
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
    fn log_filters_preserve_legacy_selection_bytes_and_request_fingerprints() {
        for fraction in ["", ".120", ".123400", ".123456700"] {
            let input = format!("1970-01-01T07:59:59{fraction}+08:00");
            let legacy_time = DateTime::parse_from_rfc3339(&input)
                .unwrap()
                .with_timezone(&Utc);
            let timestamp = serde_json::to_string(&legacy_time).unwrap();
            let operation = OperLogExportFilter::new(
                Some("operator".into()),
                Some("1".into()),
                Some(input.clone()),
                None,
            )
            .unwrap();
            let login = LoginLogExportFilter::new(
                Some("operator".into()),
                Some("1".into()),
                None,
                Some(input),
            )
            .unwrap();

            for (selection, name, begin, end, permission) in [
                (
                    ExportSelection::OperLogs(operation),
                    "oper_name",
                    timestamp.as_str(),
                    "null",
                    "system:operlog:export",
                ),
                (
                    ExportSelection::LoginLogs(login),
                    "user_name",
                    "null",
                    timestamp.as_str(),
                    "system:logininfor:export",
                ),
            ] {
                let resource = selection.resource();
                let legacy = format!(
                    r#"{{"resource":"{resource}","filter":{{"{name}":"operator","status":"1","begin_time":{begin},"end_time":{end}}}}}"#
                );
                assert_eq!(serde_json::to_string(&selection).unwrap(), legacy);
                assert_eq!(
                    serde_json::from_str::<ExportSelection>(&legacy).unwrap(),
                    selection
                );

                let sparse = legacy
                    .replace(r#""begin_time":null,"#, "")
                    .replace(r#","end_time":null"#, "");
                assert_eq!(
                    serde_json::from_str::<ExportSelection>(&sparse).unwrap(),
                    selection
                );
                let legacy_fingerprint = hex::encode(Sha256::digest(
                    [
                        b"ryframe:export-request:v1\0tenant-a\0".as_slice(),
                        &7_i64.to_be_bytes(),
                        permission.as_bytes(),
                        b"\0auth\0",
                        legacy.as_bytes(),
                    ]
                    .concat(),
                ));
                assert_eq!(
                    calculate_request_fingerprint("tenant-a", 7, permission, &selection, "auth")
                        .unwrap(),
                    legacy_fingerprint,
                );
            }
        }
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
        let now = time::macros::datetime!(2026-08-21 00:00:00 UTC);
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
    use ryframe_application::system::operations::RoleExportFilter;
    use serde::Serialize;
    use time::{
        Duration, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339,
        macros::datetime,
    };

    #[test]
    fn worker_accepts_only_current_strict_snapshot() {
        for fraction in ["", ".120", ".123400", ".123456700"] {
            let input = format!("2026-08-20T13:00:00{fraction}+01:00");
            let instant = OffsetDateTime::parse(&input, &Rfc3339).unwrap();
            let request = snapshot_request(instant);
            let legacy = legacy_snapshot_bytes(&request);
            assert_eq!(serde_json::to_vec(&request).unwrap(), legacy);
            let restored: StoredExportRequest = serde_json::from_slice(&legacy).unwrap();
            assert_eq!(restored.snapshot_at.offset(), UtcOffset::UTC);
            assert_eq!(
                restored.snapshot_at.unix_timestamp_nanos(),
                instant.unix_timestamp_nanos()
            );
            restored.validate("roles").expect("资源应匹配");
            let persisted = PersistedExportSnapshot {
                request_version: i32::from(request.request_version),
                authorization_fingerprint: &request.authorization_fingerprint,
                snapshot_at: instant,
                upper_id: request.upper_id,
                matched_rows: 12,
            };
            let wrong_time = PersistedExportSnapshot {
                snapshot_at: persisted.snapshot_at + Duration::nanoseconds(1),
                ..persisted
            };
            assert!(matches!(
                restored.validate_persisted_snapshot(wrong_time),
                Err(AppError::Validation(message)) if message == "导出请求快照时间与任务记录不一致"
            ));
            restored.validate_persisted_snapshot(persisted).unwrap();
        }

        let valid =
            serde_json::to_value(snapshot_request(datetime!(2026-08-20 12:00 UTC))).unwrap();
        let mut previous_version = valid.clone();
        previous_version["request_version"] = serde_json::json!(EXPORT_REQUEST_VERSION - 1);
        let previous: StoredExportRequest =
            serde_json::from_value(previous_version).expect("旧版本结构仍可被类型读取");
        assert!(matches!(
            previous.validate("roles"),
            Err(AppError::Validation(_))
        ));

        let old_shape = serde_json::json!({"request": {"name": "ops"}});
        assert!(serde_json::from_value::<StoredExportRequest>(old_shape).is_err());

        let mut unknown = valid;
        unknown["legacy"] = serde_json::json!(true);
        assert!(serde_json::from_value::<StoredExportRequest>(unknown).is_err());
    }

    fn snapshot_request(snapshot_at: OffsetDateTime) -> StoredExportRequest {
        StoredExportRequest {
            request_version: EXPORT_REQUEST_VERSION,
            selection: ExportSelection::Roles(RoleExportFilter::new(
                Some("ops".into()),
                None,
                Some("0".into()),
            )),
            authorization_fingerprint: "fingerprint-at-request".into(),
            snapshot_at,
            upper_id: 88,
            matched_rows: 12,
        }
    }

    fn legacy_snapshot_bytes(request: &StoredExportRequest) -> Vec<u8> {
        #[derive(Serialize)]
        struct LegacySnapshot<'a> {
            request_version: u16,
            selection: &'a ExportSelection,
            authorization_fingerprint: &'a str,
            snapshot_at: DateTime<Utc>,
            upper_id: i64,
            matched_rows: u64,
        }

        serde_json::to_vec(&LegacySnapshot {
            request_version: request.request_version,
            selection: &request.selection,
            authorization_fingerprint: &request.authorization_fingerprint,
            snapshot_at: DateTime::from_timestamp(
                request.snapshot_at.unix_timestamp(),
                request.snapshot_at.nanosecond(),
            )
            .unwrap(),
            upper_id: request.upper_id,
            matched_rows: request.matched_rows,
        })
        .unwrap()
    }

    #[test]
    fn worker_rejects_version_or_resource_mismatch() {
        let mut request = snapshot_request(datetime!(2026-08-20 12:00 UTC));
        request.request_version = EXPORT_REQUEST_VERSION + 1;
        assert!(matches!(
            request.validate("roles"),
            Err(AppError::Validation(_))
        ));

        request.request_version = EXPORT_REQUEST_VERSION;
        assert!(matches!(
            request.validate("users"),
            Err(AppError::Validation(_))
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
        assert!(matches!(old.validate(), Err(AppError::Validation(_))));
    }
}
