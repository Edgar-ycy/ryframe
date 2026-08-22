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
}

mod auth_dto {
    use serde_json::json;

    use super::{SessionContextVo, SessionUserVo, TenantBusinessDataContextVo};
    use ryframe_api::dto::fixed_value::TenantBusinessDataState;

    #[test]
    fn session_context_serializes_explicit_super_admin_marker() {
        let context = SessionContextVo {
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
    use ryframe_application::system::RoleOptionPurpose;
    use ryframe_kernel::PaginationPolicy;

    use super::{RoleOptionPurposeDto, RoleOptionQuery};

    fn parse_query(uri: &str) -> Result<RoleOptionQuery, axum::extract::rejection::QueryRejection> {
        let uri = uri.parse::<Uri>().expect("测试 URI 必须有效");
        Query::<RoleOptionQuery>::try_from_uri(&uri).map(|Query(query)| query)
    }

    #[test]
    fn role_option_query_accepts_both_explicit_purposes() {
        let user = parse_query("/?purpose=user_assignment").expect("用户分配用途应可解析");
        assert_eq!(user.purpose, RoleOptionPurposeDto::UserAssignment);
        assert_eq!(
            user.resolve(PaginationPolicy::new(10, 100))
                .expect("用户分配用途应可转换")
                .purpose,
            RoleOptionPurpose::UserAssignment
        );

        let service =
            parse_query("/?purpose=service_account_assignment").expect("服务账号分配用途应可解析");
        assert_eq!(
            service.purpose,
            RoleOptionPurposeDto::ServiceAccountAssignment
        );
        assert_eq!(
            service
                .resolve(PaginationPolicy::new(10, 100))
                .expect("服务账号分配用途应可转换")
                .purpose,
            RoleOptionPurpose::ServiceAccountAssignment
        );
    }

    #[test]
    fn role_option_query_rejects_missing_invalid_and_unknown_fields() {
        assert!(parse_query("/?q=admin").is_err());
        assert!(parse_query("/?purpose=role_code").is_err());
        assert!(parse_query("/?purpose=user_assignment&unexpected=true").is_err());
    }
}

mod auth_context {
    use ryframe_application::UserInfo;

    use super::login_actor;

    fn user(role: &str, is_super_admin: bool) -> UserInfo {
        UserInfo {
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
