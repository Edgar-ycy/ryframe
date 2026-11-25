use axum::{
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use ryframe_api::{
    dto::{auth_dto::*, export_dto::*, role_dto::*},
    handlers::{auth_handler::context::*, common_handler::*, export_handler::*},
    http::*,
    middleware::telemetry::*,
    rate_limit::*,
    request_locale::*,
    runtime::*,
};
use ryframe_kernel::AppResult as KernelAppResult;
use ryframe_kernel::*;

#[test]
fn navigation_created_at_preserves_utc_json_and_schema() {
    use ryframe_api::dto::public_dto::{MenuVo, PermissionVo};
    use ryframe_application::ports::system::{MenuRecord, PermissionRecord};
    use ryframe_application::system::identity::{MenuVo as Menu, PermissionVo as Permission};
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
    for (nanos, expected) in [
        (0, "1969-12-31T23:59:59Z"),
        (120_000_000, "1969-12-31T23:59:59.120Z"),
        (123_400_000, "1969-12-31T23:59:59.123400Z"),
        (123_456_700, "1969-12-31T23:59:59.123456700Z"),
    ] {
        let created_at = OffsetDateTime::from_unix_timestamp(-1)
            .unwrap()
            .replace_nanosecond(nanos)
            .unwrap()
            .to_offset(offset);
        let menu = Menu::from(MenuRecord {
            id: 1,
            name: "菜单".into(),
            parent_id: None,
            menu_type: "C".into(),
            perm_id: Some(2),
            route_key: None,
            icon: None,
            sort: 0,
            visible: true,
            status: "1".into(),
            remark: None,
            created_at,
            updated_at: Default::default(),
        });
        let permission = Permission::from(PermissionRecord {
            id: 2,
            name: "权限".into(),
            code: "system:menu:list".into(),
            parent_id: None,
            perm_type: "menu".into(),
            icon: None,
            sort: 0,
            status: "1".into(),
            created_at,
            updated_at: Default::default(),
        });
        for response in [
            serde_json::to_value(&menu).unwrap(),
            serde_json::to_value(&permission).unwrap(),
            serde_json::to_value(MenuVo::try_from(menu).unwrap()).unwrap(),
            serde_json::to_value(PermissionVo::try_from(permission).unwrap()).unwrap(),
        ] {
            assert_eq!(response["created_at"], expected);
        }
    }
    for schema in [MenuVo::schema(), PermissionVo::schema()] {
        let schema = serde_json::to_value(schema).unwrap();
        assert_eq!(schema["properties"]["created_at"]["type"], "string");
        assert_eq!(schema["properties"]["created_at"]["format"], "date-time");
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!("created_at"))
        );
    }
}

#[test]
fn role_created_at_preserves_utc_json_and_schema() {
    use ryframe_api::dto::public_dto::RoleVo;
    use ryframe_application::{
        ports::{system::RoleRecord, utc_timestamp},
        system::identity::RoleVo as Role,
    };
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
    for (nanos, expected) in [
        (0, "1969-12-31T23:59:59Z"),
        (120_000_000, "1969-12-31T23:59:59.120Z"),
        (123_400_000, "1969-12-31T23:59:59.123400Z"),
        (123_456_700, "1969-12-31T23:59:59.123456700Z"),
    ] {
        let role = Role::from(RoleRecord {
            id: 1,
            name: "工单审核员".into(),
            code: "reviewer".into(),
            is_super: 0,
            data_scope: "1".into(),
            status: "1".into(),
            sort: 0,
            remark: None,
            created_at: OffsetDateTime::from_unix_timestamp(-1)
                .unwrap()
                .replace_nanosecond(nanos)
                .unwrap()
                .to_offset(offset),
            updated_at: Default::default(),
        });
        assert_eq!(serde_json::to_value(&role).unwrap()["created_at"], expected);
        assert_eq!(
            utc_timestamp::format_with_offset(&role.created_at).unwrap(),
            format!("{}+00:00", expected.strip_suffix('Z').unwrap())
        );
        let response = serde_json::to_value(RoleVo::from(role)).unwrap();
        assert_eq!(response["created_at"], expected);
    }

    let schema = serde_json::to_value(RoleVo::schema()).unwrap();
    assert_eq!(
        schema["properties"]["created_at"],
        json!({"type":"string","format":"date-time"})
    );
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("created_at"))
    );
}

#[test]
fn department_created_at_preserves_utc_json_and_schema() {
    use ryframe_api::dto::public_dto::DeptVo;
    use ryframe_application::{ports::system::DeptRecord, system::identity::DeptVo as Dept};
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
    for (nanos, expected) in [
        (0, "1969-12-31T23:59:59Z"),
        (120_000_000, "1969-12-31T23:59:59.120Z"),
        (123_400_000, "1969-12-31T23:59:59.123400Z"),
        (123_456_700, "1969-12-31T23:59:59.123456700Z"),
    ] {
        let dept = Dept::from(DeptRecord {
            id: 1,
            name: "研发部".into(),
            parent_id: None,
            ancestors: "0".into(),
            sort: 0,
            status: "1".into(),
            remark: None,
            created_at: OffsetDateTime::from_unix_timestamp(-1)
                .unwrap()
                .replace_nanosecond(nanos)
                .unwrap()
                .to_offset(offset),
            updated_at: Default::default(),
        });
        assert_eq!(serde_json::to_value(&dept).unwrap()["created_at"], expected);
        let response = serde_json::to_value(DeptVo::from(dept)).unwrap();
        assert_eq!(response["created_at"], expected);
    }

    let schema = serde_json::to_value(DeptVo::schema()).unwrap();
    assert_eq!(schema["properties"]["created_at"]["type"], "string");
    assert_eq!(schema["properties"]["created_at"]["format"], "date-time");
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("created_at"))
    );
}

#[test]
fn schedule_preview_preserves_utc_precision_local_text_and_required_schema() {
    use ryframe_api::dto::public_dto::{JobScheduleOccurrence, JobSchedulePreview};
    use ryframe_application::{
        JobScheduleOccurrence as ServiceOccurrence, JobSchedulePreview as ServicePreview,
    };
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    let local_text = "1970-01-02T05:00:00-00:45";
    let non_utc = UtcOffset::from_hms(8, 0, 0).unwrap();
    for (nanos, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        let instant = OffsetDateTime::from_unix_timestamp(107_070)
            .unwrap()
            .replace_nanosecond(nanos)
            .unwrap()
            .to_offset(non_utc);
        let service_preview = ServicePreview {
            calculated_at: instant,
            timezone: "Africa/Monrovia".into(),
            occurrences: vec![ServiceOccurrence {
                utc: instant,
                schedule_time: local_text.into(),
            }],
        };
        let service_json = serde_json::to_value(&service_preview).unwrap();
        let body = serde_json::to_value(JobSchedulePreview::from(service_preview)).unwrap();
        assert_eq!(body, service_json);
        let expected = format!("1970-01-02T05:44:30{fraction}Z");
        assert_eq!(body["calculated_at"], expected);
        assert_eq!(body["occurrences"][0]["utc"], expected);
        assert_eq!(body["occurrences"][0]["schedule_time"], local_text);
        assert_eq!(body["timezone"], "Africa/Monrovia");
    }
    for (schema, field) in [
        (JobScheduleOccurrence::schema(), "utc"),
        (JobSchedulePreview::schema(), "calculated_at"),
    ] {
        let schema = serde_json::to_value(schema).unwrap();
        let property = &schema["properties"][field];
        assert_eq!(property["type"], json!("string"));
        assert_eq!(property["format"], "date-time");
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .contains(&json!(field))
        );
    }
}

#[test]
fn data_target_verification_preserves_nullable_utc_precision_and_schema() {
    use ryframe_api::dto::tenant_data_dto::DataTargetDetail;
    use ryframe_application::system::platform::{
        DataTargetDetail as ServiceDetail, DataTargetSummary as ServiceSummary,
    };
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    let response = |last_verified_at| {
        let detail = ServiceDetail {
            target: ServiceSummary {
                key: "shared".into(),
                display_name: None,
                mode: "shared".into(),
                kind: "mysql".into(),
                region: None,
                health: "verified".into(),
                schema_fingerprint: None,
                connected: true,
                pool_max_connections: Some(10),
                active_leases: 0,
                eligible: true,
                reasons: Vec::new(),
            },
            last_verified_at,
            reserved_connections: 0,
            max_total_connections: 10,
            open_targets: 1,
            opening_targets: 0,
        };
        let service_json = serde_json::to_value(&detail).unwrap();
        let body = serde_json::to_value(DataTargetDetail::try_from(detail).unwrap()).unwrap();
        assert_eq!(body, service_json);
        body
    };
    assert!(response(None)["last_verified_at"].is_null());
    for (nanos, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        let value = OffsetDateTime::from_unix_timestamp(-1)
            .unwrap()
            .replace_nanosecond(nanos)
            .unwrap()
            .to_offset(UtcOffset::from_hms(8, 0, 0).unwrap());
        assert_eq!(
            response(Some(value))["last_verified_at"],
            format!("1969-12-31T23:59:59{fraction}Z")
        );
    }
    let schema = serde_json::to_value(DataTargetDetail::schema()).unwrap();
    let fields = &schema["allOf"][1];
    let property = &fields["properties"]["last_verified_at"];
    assert_eq!(property["type"], json!(["string", "null"]));
    assert_eq!(property["format"], "date-time");
    assert!(
        !fields["required"]
            .as_array()
            .unwrap()
            .contains(&json!("last_verified_at"))
    );
}

#[test]
fn data_retention_run_preserves_utc_json_nullability_and_schema() {
    use chrono::DateTime;
    use ryframe_api::dto::public_dto::DataRetentionRunVo;
    use ryframe_application::{
        ports::retention::RetentionRunRecord, system::operations::DataRetentionRunVo as ServiceRun,
    };
    use serde_json::{Value, json};
    use utoipa::PartialSchema;

    let mut record = RetentionRunRecord {
        id: 1,
        background_job_id: 2,
        trigger_kind: RetentionRunRecord::TRIGGER_MANUAL.into(),
        status: RetentionRunRecord::STATUS_PENDING.into(),
        policy_snapshot: json!({}),
        eligible_counts: json!({}),
        deleted_counts: json!({}),
        remaining_counts: json!({}),
        requested_by: None,
        error_summary: None,
        started_at: None,
        completed_at: None,
        created_at: DateTime::from_timestamp(-1, 123_400_000).unwrap(),
        updated_at: DateTime::from_timestamp(0, 123_456_700).unwrap(),
    };
    let pending = ServiceRun::try_from(record.clone()).unwrap();
    for response in [
        serde_json::to_value(&pending).unwrap(),
        serde_json::to_value(DataRetentionRunVo::from(pending)).unwrap(),
    ] {
        assert_eq!(response.get("started_at"), Some(&Value::Null));
        assert_eq!(response.get("completed_at"), Some(&Value::Null));
    }

    record.status = RetentionRunRecord::STATUS_SUCCEEDED.into();
    record.started_at = DateTime::from_timestamp(-1, 0);
    record.completed_at = DateTime::from_timestamp(0, 120_000_000);
    let completed = ServiceRun::try_from(record.clone()).unwrap();
    for response in [
        serde_json::to_value(&completed).unwrap(),
        serde_json::to_value(DataRetentionRunVo::from(completed)).unwrap(),
    ] {
        assert_eq!(response["started_at"], "1969-12-31T23:59:59Z");
        assert_eq!(response["completed_at"], "1970-01-01T00:00:00.120Z");
        assert_eq!(response["created_at"], "1969-12-31T23:59:59.123400Z");
        assert_eq!(response["updated_at"], "1970-01-01T00:00:00.123456700Z");
    }

    let schema = serde_json::to_value(DataRetentionRunVo::schema()).unwrap();
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

    record.completed_at = Some(DateTime::from_timestamp(-1, 1_123_456_000).unwrap());
    assert!(matches!(
        ServiceRun::try_from(record),
        Err(AppError::Internal(message)) if message.contains("completed_at")
    ));
}

#[test]
fn user_import_row_preserves_created_at_utc_precision_and_required_schema() {
    use ryframe_api::dto::public_dto::UserImportRowVo;
    use ryframe_application::{
        ports::users::UserImportRowRecord, system::identity::UserImportRowVo as ServiceRow,
    };
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    for (nanos, fraction) in [
        (0, ""),
        (120_000_000, ".120"),
        (123_400_000, ".123400"),
        (123_456_700, ".123456700"),
    ] {
        let created_at = OffsetDateTime::from_unix_timestamp(-1)
            .unwrap()
            .replace_nanosecond(nanos)
            .unwrap()
            .to_offset(UtcOffset::from_hms(8, 0, 0).unwrap());
        let service_row = ServiceRow::from(UserImportRowRecord {
            row_number: 3,
            username: "tester".into(),
            outcome: "skipped".into(),
            code: "already_exists".into(),
            message: "用户已存在".into(),
            created_at,
        });
        let service_json = serde_json::to_value(&service_row).unwrap();
        let body = serde_json::to_value(UserImportRowVo::from(service_row)).unwrap();
        assert_eq!(body, service_json);
        assert_eq!(
            body["created_at"],
            format!("1969-12-31T23:59:59{fraction}Z")
        );
        assert_eq!(body["row_number"], 3);
        assert_eq!(body["username"], "tester");
        assert_eq!(body["outcome"], "skipped");
        assert_eq!(body["code"], "already_exists");
        assert_eq!(body["message"], "用户已存在");
    }
    let schema = serde_json::to_value(UserImportRowVo::schema()).unwrap();
    let property = &schema["properties"]["created_at"];
    assert_eq!(property["type"], json!("string"));
    assert_eq!(property["format"], "date-time");
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("created_at"))
    );
}

mod http {
    use super::*;

    #[test]
    fn export_limit_error_is_413_with_safe_details() {
        let error = AppError::ExportRowLimitExceeded {
            matched_rows: 500_001,
            limit: 500_000,
        };
        assert_eq!(
            HttpAppError(AppError::ExportRowLimitExceeded {
                matched_rows: 500_001,
                limit: 500_000,
            })
            .into_response()
            .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            public_error_details(&error),
            Some(serde_json::json!({
                "matched_rows": 500_001,
                "limit": 500_000,
            }))
        );
    }

    #[test]
    fn no_matching_rows_uses_stable_bad_request_code() {
        let error = AppError::ExportNoMatchingRows("没有匹配记录".into());
        assert_eq!(error.error_code().as_str(), "EXPORT_NO_MATCHING_ROWS");
        assert_eq!(
            HttpAppError(error).into_response().status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn client_errors_only_publish_allowlisted_reasons() {
        assert_eq!(
            public_error_details(&AppError::Validation("验证码错误或已过期".into())),
            Some(serde_json::json!({
                "reason": "captcha_invalid_or_expired",
                "field": "captcha",
            }))
        );
        assert_eq!(
            public_error_details(&AppError::Validation(
                "单租户模式只允许使用 system 租户".into(),
            )),
            Some(serde_json::json!({
                "reason": "fixed_tenant_required",
                "field": "tenant_id",
            }))
        );
        assert_eq!(
            public_error_details(&AppError::Validation("page must be greater than 0".into())),
            Some(serde_json::json!({"reason": "invalid_parameters"}))
        );
        assert_eq!(
            public_error_details(&AppError::Authorization(
                "全局业务仅允许系统租户访问".into(),
            )),
            Some(serde_json::json!({"reason": "system_tenant_required"}))
        );
        assert_eq!(
            public_error_details(&AppError::Authentication("用户名存在但密码错误".into(),)),
            None
        );
        assert_eq!(
            public_error_details(&AppError::Database(
                "mysql://admin:secret@database/control".into(),
            )),
            None
        );
    }
}

mod auth_dto {
    use serde_json::json;

    use super::{SessionContextVo, SessionUserVo, TenantBusinessDataContextVo};
    use ryframe_api::dto::fixed_value::TenantBusinessDataState;

    #[test]
    fn session_context_serializes_explicit_super_admin_marker() {
        let context = SessionContextVo {
            password_change_required: false,
            user: SessionUserVo {
                id: "1".into(),
                tenant_id: "tenant-a".into(),
                tenant_name: "租户甲".into(),
                dept_name: None,
                username: "tester".into(),
                nickname: "测试用户".into(),
                email: String::new(),
                phone: String::new(),
                avatar: None,
                preferred_locale: None,
            },
            is_super_admin: true,
            roles: vec!["ordinary".into()],
            permissions: Vec::new(),
            authorization_epoch: "1".into(),
            runtime_epoch: "1".into(),
            capabilities: Vec::new(),
            business_data: TenantBusinessDataContextVo {
                state: TenantBusinessDataState::Active,
                placement_generation: "1".into(),
            },
            menus: Vec::new(),
        };

        let value = serde_json::to_value(context).expect("会话上下文应能序列化");
        assert_eq!(value["is_super_admin"], json!(true));
    }
}

mod export_dto {
    use super::*;

    #[test]
    fn all_export_requests_require_strict_envelope() {
        macro_rules! assert_contract {
            ($request:ty, $filter:expr) => {{
                let valid = serde_json::json!({"filter": $filter, "confirm_all": false});
                serde_json::from_value::<$request>(valid).expect("统一包络应可解析");

                let unknown = serde_json::json!({
                    "filter": $filter,
                    "confirm_all": false,
                    "legacy": true
                });
                assert!(serde_json::from_value::<$request>(unknown).is_err());

                let old_shape = $filter;
                assert!(serde_json::from_value::<$request>(old_shape).is_err());
            }};
        }

        assert_contract!(UserExportRequestDto, serde_json::json!({}));
        assert_contract!(RoleExportRequestDto, serde_json::json!({}));
        assert_contract!(PostExportRequestDto, serde_json::json!({}));
        assert_contract!(ConfigExportRequestDto, serde_json::json!({}));
        assert_contract!(DictTypeExportRequestDto, serde_json::json!({}));
        assert_contract!(OperLogExportRequestDto, serde_json::json!({}));
        assert_contract!(LoginLogExportRequestDto, serde_json::json!({}));
    }

    #[test]
    fn export_filters_reject_pagination_and_unknown_fields() {
        let page = serde_json::json!({
            "filter": {"name": "ops", "page": 2},
            "confirm_all": false
        });
        assert!(serde_json::from_value::<RoleExportRequestDto>(page).is_err());

        let page_size = serde_json::json!({
            "filter": {"name": "ops", "page_size": 100},
            "confirm_all": false
        });
        assert!(serde_json::from_value::<RoleExportRequestDto>(page_size).is_err());

        let wrong_log_field = serde_json::json!({
            "filter": {"name": "operator"},
            "confirm_all": false
        });
        assert!(serde_json::from_value::<OperLogExportRequestDto>(wrong_log_field).is_err());
    }

    #[test]
    fn export_selected_ids_use_strict_positive_string_ids() {
        for ids in [
            serde_json::json!(["0"]),
            serde_json::json!(["-1"]),
            serde_json::json!(["9223372036854775808"]),
        ] {
            let dto: PostExportRequestDto = serde_json::from_value(serde_json::json!({
                "filter": {}, "confirm_all": false, "ids": ids,
            }))
            .expect("结构正确");
            assert!(dto.into_selection().is_err());
        }
        assert!(
            serde_json::from_value::<PostExportRequestDto>(serde_json::json!({
                "filter": {}, "confirm_all": false, "ids": [1],
            }))
            .is_err()
        );
        let dto: PostExportRequestDto = serde_json::from_value(serde_json::json!({
            "filter": {}, "confirm_all": false, "ids": ["9007199254740993", "2"],
        }))
        .expect("字符串 ID");
        let (selection, confirm) = dto.into_selection().expect("有效 ID");
        assert_eq!(selection.selected_ids(), &[2, 9007199254740993]);
        assert!(!confirm);
        assert!(!selection.is_empty());
    }

    #[test]
    fn mapping_preserves_zero_and_rejects_invalid_time_before_enqueue() {
        let user: UserExportRequestDto = serde_json::from_value(serde_json::json!({
            "filter": {"username": " ", "dept_id": "0", "status": "0"},
            "confirm_all": false
        }))
        .expect("用户请求应可解析");
        let (selection, _) = user.into_selection().expect("数值零应可映射");
        assert!(!selection.is_empty());

        let log: OperLogExportRequestDto = serde_json::from_value(serde_json::json!({
            "filter": {
                "oper_name": "operator",
                "begin_time": "2026-08-20T04:00:00Z",
                "end_time": "2026-08-20T03:00:00Z"
            },
            "confirm_all": false
        }))
        .expect("DTO 解析不应隐藏时间错误");
        assert!(matches!(log.into_selection(), Err(AppError::Validation(_))));
    }

    #[test]
    fn deletion_command_is_strict_and_preserves_ids_for_application_normalization() {
        let request: DeleteExportJobsDto = serde_json::from_value(serde_json::json!({
            "ids": ["9", "3", "9"]
        }))
        .expect("严格删除命令应可解析");
        assert_eq!(request.into_ids().expect("ID 应有效"), vec![9, 3, 9]);

        assert!(
            serde_json::from_value::<DeleteExportJobsDto>(serde_json::json!({
                "ids": ["1"],
                "legacy": true
            }))
            .is_err()
        );
        let invalid: DeleteExportJobsDto =
            serde_json::from_value(serde_json::json!({"ids": ["0"]})).expect("结构应先解析");
        assert!(invalid.into_ids().is_err());
    }
}

mod role_dto {
    use axum::{extract::Query, http::Uri};
    use ryframe_application::system::identity::RoleOptionPurpose;
    use ryframe_kernel::PaginationPolicy;

    use super::{RoleOptionPurposeDto, RoleOptionQuery};

    fn parse_query(uri: &str) -> Result<RoleOptionQuery, axum::extract::rejection::QueryRejection> {
        let uri = uri.parse::<Uri>().expect("测试 URI 必须有效");
        Query::<RoleOptionQuery>::try_from_uri(&uri).map(|Query(query)| query)
    }

    #[test]
    fn role_option_query_accepts_user_assignment() {
        let user = parse_query("/?purpose=user_assignment").expect("用户分配用途应可解析");
        assert_eq!(user.purpose, RoleOptionPurposeDto::UserAssignment);
        assert_eq!(
            user.resolve(PaginationPolicy::new(10, 100))
                .expect("用户分配用途应可转换")
                .purpose,
            RoleOptionPurpose::UserAssignment
        );
    }

    #[test]
    fn role_option_query_rejects_missing_invalid_and_unknown_fields() {
        assert!(parse_query("/?q=admin").is_err());
        assert!(parse_query("/?purpose=role_code").is_err());
        assert!(parse_query("/?purpose=service_account_assignment").is_err());
        assert!(parse_query("/?purpose=user_assignment&unexpected=true").is_err());
    }
}

mod auth_context {
    use ryframe_application::UserInfo;

    use super::login_actor;

    fn user(role: &str, is_super_admin: bool) -> UserInfo {
        UserInfo {
            password_change_required: false,
            id: "1".into(),
            tenant_id: "tenant-a".into(),
            tenant_name: "租户甲".into(),
            dept_name: None,
            username: "tester".into(),
            nickname: "测试用户".into(),
            email: String::new(),
            phone: String::new(),
            avatar: None,
            preferred_locale: None,
            is_super_admin,
            roles: vec![role.into()],
            perms: Vec::new(),
        }
    }

    #[test]
    fn login_actor_does_not_infer_super_admin_from_role_code() {
        assert!(!login_actor(1, &user("admin", false)).is_super_admin);
        assert!(login_actor(1, &user("ordinary", true)).is_super_admin);
    }
}

mod common_handler {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Default)]
    struct RecordingCircuitBreaker {
        successes: AtomicUsize,
        failures: AtomicUsize,
    }

    impl UploadCircuitBreaker for RecordingCircuitBreaker {
        fn allow_request(&self) -> bool {
            true
        }

        fn record_success(&self) {
            self.successes.fetch_add(1, Ordering::Relaxed);
        }

        fn record_failure(&self) {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }

        fn state_label(&self) -> &'static str {
            "Closed"
        }
    }

    #[test]
    fn records_only_infrastructure_failures() {
        let breaker = RecordingCircuitBreaker::default();

        record_upload_result(&breaker, &KernelAppResult::<()>::Ok(()));
        record_upload_result(
            &breaker,
            &KernelAppResult::<()>::Err(AppError::Database("连接失败".into())),
        );
        record_upload_result(
            &breaker,
            &KernelAppResult::<()>::Err(AppError::ServiceUnavailable("存储不可用".into())),
        );
        record_upload_result(
            &breaker,
            &KernelAppResult::<()>::Err(AppError::Validation("文件格式错误".into())),
        );

        assert_eq!(breaker.successes.load(Ordering::Relaxed), 1);
        assert_eq!(breaker.failures.load(Ordering::Relaxed), 2);
    }
}

mod export_handler {
    use axum::http::HeaderValue;

    use super::*;

    #[test]
    fn deletion_requires_a_bounded_visible_ascii_idempotency_key() {
        assert!(require_idempotency_key(&HeaderMap::new()).is_err());

        let mut headers = HeaderMap::new();
        headers.insert(
            "Idempotency-Key",
            HeaderValue::from_static("delete-export-01"),
        );
        require_idempotency_key(&headers).expect("有效幂等键应通过");

        headers.insert("Idempotency-Key", HeaderValue::from_static("has space"));
        assert!(require_idempotency_key(&headers).is_err());
    }
}

mod request_locale {
    use super::*;

    #[test]
    fn locale_negotiation_honors_quality_and_fallbacks() {
        assert_eq!(
            negotiate_locale(Some("zh-CN;q=0.5, en-US;q=0.9"), Some("zh-CN")),
            Locale::EnUs
        );
        assert_eq!(negotiate_locale(None, Some("en-GB")), Locale::EnUs);
        assert_eq!(negotiate_locale(Some("fr-FR"), None), Locale::DEFAULT);
    }
}

mod telemetry {
    use super::{HttpResponseClass, classify_http_response};

    #[test]
    fn response_classes_follow_http_status_ranges() {
        assert_eq!(classify_http_response(204), HttpResponseClass::Success);
        assert_eq!(classify_http_response(404), HttpResponseClass::ClientError);
        assert_eq!(classify_http_response(503), HttpResponseClass::ServerError);
    }
}

mod rate_limit {
    use std::net::{IpAddr, Ipv4Addr};

    use super::{api_client_key, tenant_key, tenant_user_key};

    #[test]
    fn rate_limit_keys_preserve_existing_namespaces() {
        assert_eq!(tenant_key("tenant-a"), "tenant:tenant-a");
        assert_eq!(tenant_user_key("tenant-a", "42"), "tenant_user:tenant-a:42");
        assert_eq!(
            api_client_key("/api/v1/users", IpAddr::V4(Ipv4Addr::LOCALHOST)),
            "api:/api/v1/users:ip:127.0.0.1"
        );
    }
}

#[test]
fn config_created_at_preserves_cache_json_and_required_api_schema() {
    use ryframe_api::dto::public_dto::ConfigVo;
    use ryframe_application::{
        ports::{system::ConfigRecord, utc_timestamp},
        system::content::ConfigVo as Config,
    };
    use serde_json::json;
    use time::{OffsetDateTime, UtcOffset};
    use utoipa::PartialSchema;

    let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
    for (nanos, timestamp) in [
        (0, "1969-12-31T23:59:59Z"),
        (120_000_000, "1969-12-31T23:59:59.120Z"),
        (123_400_000, "1969-12-31T23:59:59.123400Z"),
        (123_456_700, "1969-12-31T23:59:59.123456700Z"),
    ] {
        let created_at = OffsetDateTime::from_unix_timestamp(-1)
            .unwrap()
            .replace_nanosecond(nanos)
            .unwrap()
            .to_offset(offset);
        let config = Config::from(ConfigRecord {
            id: 1,
            name: "界面配置".into(),
            key: "ui.theme".into(),
            value: "dark".into(),
            portable: true,
            remark: Some("默认主题".into()),
            created_at,
            updated_at: Default::default(),
        });
        let cached_json = json!({
            "id": "1",
            "name": "界面配置",
            "key": "ui.theme",
            "value": "dark",
            "portable": true,
            "remark": "默认主题",
            "created_at": timestamp,
        });
        assert_eq!(serde_json::to_value(&config).unwrap(), cached_json);
        assert_eq!(
            serde_json::to_value(ConfigVo::from(config)).unwrap(),
            cached_json
        );

        let restored = serde_json::from_value::<Config>(cached_json).unwrap();
        assert_eq!(restored.created_at, created_at.to_offset(UtcOffset::UTC));
        assert_eq!(restored.created_at.offset(), UtcOffset::UTC);
        let previous_export = chrono::DateTime::from_timestamp(-1, nanos)
            .unwrap()
            .to_rfc3339();
        assert_eq!(
            utc_timestamp::format_with_offset(&restored.created_at).unwrap(),
            previous_export
        );
    }

    let schema = serde_json::to_value(ConfigVo::schema()).unwrap();
    assert_eq!(schema["properties"]["created_at"]["type"], "string");
    assert_eq!(schema["properties"]["created_at"]["format"], "date-time");
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "created_at")
    );
}
