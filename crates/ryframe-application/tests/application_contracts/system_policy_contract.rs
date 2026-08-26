use std::collections::{BTreeMap, HashSet};

use chrono::{Duration, TimeZone, Utc};
use ryframe_application::{
    jobs::{normalize_job_status_filter, public_job_error},
    ports::{
        auth::{PASSWORD_RESET_STATUS_PENDING, PasswordResetUserState},
        authorization::DiagnosticMenuRecord,
        files::{
            ArtifactStoreError, ArtifactStoreErrorKind, FILE_DEL_FLAG_NORMAL,
            FILE_UPLOAD_STATUS_CLEANUP, FILE_UPLOAD_STATUS_PENDING, FileCleanupRecord,
        },
        jobs::JobScheduleRecord,
        product::{ProductCapabilityRecord, ProductVersionRecord},
        tenants::TenantCapacityRecord,
        users::{
            USER_STATUS_DISABLED, USER_STATUS_MUST_RESET_PASSWORD, USER_STATUS_NORMAL,
            USER_STATUS_PENDING_ACTIVATION, UserAssignmentRole, UserAssignmentState,
            UserImportDepartmentRecord, UserImportSourceRecord, UserImportSourceState,
        },
    },
    system::{
        MessageText, ProductService, SERVICE_ACCOUNTS_CAPABILITY, ServiceCapabilityDescriptor,
        dept::rewrite_descendant_ancestors,
        dict::dict_cache_key,
        file::{
            ExpiredReservationPlan, map_storage_read_error, map_storage_write_error,
            plan_expired_reservation,
        },
        menu::normalize_route_key,
        permission::{ensure_tenant_permission_code_boundary, is_platform_permission_code},
        profile::normalize_preferred_locale,
        service_account::common_capabilities,
        tenant::{
            data_migration::rolling_digest,
            usage::{expiration_status, percentage_basis_points, quota_status},
        },
        user::{
            ensure_not_super, ensure_pending, normalize_ids, password_reset_next_status,
            validate_assignment_state, validate_manageable_status,
        },
        user_import::{available_department_paths, validate_import_source},
        validate_message_text_pair,
    },
};
use ryframe_kernel::{ActorContext, AppError, DataScope};

fn diagnostic_menu(status: &str, menu_type: &str, perm_id: Option<i64>) -> DiagnosticMenuRecord {
    DiagnosticMenuRecord {
        id: 1,
        parent_id: None,
        name: "测试菜单".to_owned(),
        route_key: Some("test".to_owned()),
        perm_id,
        menu_type: menu_type.to_owned(),
        status: status.to_owned(),
        visible: true,
    }
}

fn file_reservation(status: &str, expires_at: chrono::DateTime<Utc>) -> FileCleanupRecord {
    FileCleanupRecord {
        id: 1,
        tenant_id: "tenant-a".to_owned(),
        bucket: "uploads".to_owned(),
        storage_path: "tenant-a/object".to_owned(),
        upload_status: status.to_owned(),
        reservation_token: Some("token".to_owned()),
        reservation_expires_at: Some(expires_at),
        del_flag: FILE_DEL_FLAG_NORMAL.to_owned(),
    }
}

fn tenant(expire_at: Option<chrono::DateTime<Utc>>) -> TenantCapacityRecord {
    TenantCapacityRecord {
        tenant_id: "tenant-a".into(),
        name: "测试租户".into(),
        domain: None,
        status: "0".into(),
        expire_at,
        max_users: 100,
        max_roles: 20,
        max_storage_mb: 1024,
        max_requests_per_min: 1000,
    }
}

fn actor() -> ActorContext {
    ActorContext {
        user_id: 1,
        tenant_id: "tenant-a".into(),
        username: "tester".into(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::All,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: true,
    }
}

fn department(
    id: i64,
    name: &str,
    parent_id: Option<i64>,
    ancestors: &str,
    status: &str,
) -> UserImportDepartmentRecord {
    UserImportDepartmentRecord {
        id,
        name: name.into(),
        parent_id,
        ancestors: ancestors.into(),
        status: status.into(),
    }
}

fn import_source(state: UserImportSourceState) -> UserImportSourceRecord {
    UserImportSourceRecord {
        bucket: "imports".into(),
        sha256: "a".repeat(64),
        state,
    }
}

fn assignment_state(roles: Vec<UserAssignmentRole>) -> UserAssignmentState {
    UserAssignmentState {
        department_exists: true,
        roles,
    }
}

#[test]
fn job_status_and_public_error_rules_are_application_owned() {
    assert_eq!(
        normalize_job_status_filter(Some("dead".into())).unwrap(),
        Some("dead".into())
    );
    assert!(normalize_job_status_filter(Some("cancelled".into())).is_err());
    assert_eq!(
        public_job_error("system.tenant_config.apply", Some("secret".into())).as_deref(),
        Some("配置应用失败，请稍后重试或联系管理员")
    );
}

#[test]
fn schedule_record_maps_every_public_field() {
    let created_at = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
    let updated_at = Utc.with_ymd_and_hms(2026, 8, 21, 2, 3, 4).unwrap();
    let next_run_at = Utc.with_ymd_and_hms(2026, 8, 22, 0, 0, 0).unwrap();
    let last_run_at = Utc.with_ymd_and_hms(2026, 8, 20, 0, 0, 0).unwrap();
    let schedule = ryframe_application::JobScheduleVo::from(JobScheduleRecord {
        id: 7,
        tenant_id: "tenant-a".to_owned(),
        name: "日报".to_owned(),
        handler_key: "report.daily".to_owned(),
        cron_expression: "0 0 0 * * *".to_owned(),
        timezone: "Asia/Shanghai".to_owned(),
        enabled: true,
        misfire_policy: "fire_once".to_owned(),
        concurrency_policy: "forbid".to_owned(),
        max_runtime_seconds: 300,
        next_run_at: Some(next_run_at),
        last_run_at: Some(last_run_at),
        version: 9,
        created_at,
        updated_at,
        deleted: false,
    });
    assert_eq!(schedule.id, "7");
    assert_eq!(schedule.name, "日报");
    assert_eq!(schedule.handler_key, "report.daily");
    assert_eq!(schedule.cron_expression, "0 0 0 * * *");
    assert_eq!(schedule.timezone, "Asia/Shanghai");
    assert!(schedule.enabled);
    assert_eq!(schedule.misfire_policy, "fire_once");
    assert_eq!(schedule.concurrency_policy, "forbid");
    assert_eq!(schedule.max_runtime_seconds, 300);
    assert_eq!(schedule.next_run_at, Some(next_run_at));
    assert_eq!(schedule.last_run_at, Some(last_run_at));
    assert_eq!(schedule.version, 9);
    assert_eq!(schedule.created_at, created_at);
    assert_eq!(schedule.updated_at, updated_at);
}

#[test]
fn diagnostic_reason_prefers_identity_state_and_distinguishes_configuration() {
    let disabled = diagnostic_menu("0", "C", None);
    assert_eq!(
        ryframe_application::system::authorization_diagnostic::menu_inaccessible_reason(
            &disabled, None, false, false, false,
        )
        .as_deref(),
        Some("tenant_unavailable")
    );
    assert_eq!(
        ryframe_application::system::authorization_diagnostic::menu_inaccessible_reason(
            &disabled, None, true, false, false,
        )
        .as_deref(),
        Some("user_disabled")
    );
    assert_eq!(
        ryframe_application::system::authorization_diagnostic::menu_inaccessible_reason(
            &disabled, None, true, true, false,
        )
        .as_deref(),
        Some("menu_disabled")
    );
    assert_eq!(
        ryframe_application::system::authorization_diagnostic::menu_inaccessible_reason(
            &diagnostic_menu("1", "M", None),
            None,
            true,
            true,
            false,
        )
        .as_deref(),
        Some("no_accessible_child")
    );
    assert_eq!(
        ryframe_application::system::authorization_diagnostic::menu_inaccessible_reason(
            &diagnostic_menu("1", "C", Some(9)),
            None,
            true,
            true,
            false,
        )
        .as_deref(),
        Some("invalid_permission_reference")
    );
}

#[test]
fn descendant_path_rewrite_preserves_suffix_and_rejects_mismatch() {
    assert_eq!(
        rewrite_descendant_ancestors("0,10,20,30", "0,10", "0,40").unwrap(),
        "0,40,20,30"
    );
    assert!(rewrite_descendant_ancestors("0,11,20", "0,10", "0,40").is_err());
}

#[test]
fn dictionary_cache_key_is_tenant_scoped() {
    assert_eq!(
        dict_cache_key("tenant-a", "sys.status"),
        "sys_dict:data:tenant-a:sys.status"
    );
    assert_ne!(
        dict_cache_key("tenant-a", "sys.status"),
        dict_cache_key("tenant-b", "sys.status")
    );
}

#[test]
fn storage_errors_map_to_stable_application_errors() {
    for error in [
        map_storage_write_error(ArtifactStoreError::new(
            ArtifactStoreErrorKind::Misconfigured,
            "list",
        )),
        map_storage_read_error(ArtifactStoreError::new(
            ArtifactStoreErrorKind::Misconfigured,
            "list",
        )),
    ] {
        assert!(matches!(error, AppError::Internal(_)));
    }
    for error in [
        map_storage_write_error(ArtifactStoreError::new(
            ArtifactStoreErrorKind::Unavailable,
            "truncated",
        )),
        map_storage_read_error(ArtifactStoreError::new(
            ArtifactStoreErrorKind::Unavailable,
            "truncated",
        )),
    ] {
        assert!(matches!(error, AppError::ServiceUnavailable(_)));
    }
}

#[test]
fn expired_file_reservations_follow_cleanup_lifecycle() {
    let now = Utc::now();
    let grace = Duration::minutes(5);
    let pending = file_reservation(FILE_UPLOAD_STATUS_PENDING, now - Duration::seconds(1));
    assert_eq!(
        plan_expired_reservation(&pending, now, grace),
        Some(ExpiredReservationPlan::BeginCleanup {
            cleanup_after: now + grace,
        })
    );
    let cleanup = file_reservation(FILE_UPLOAD_STATUS_CLEANUP, now - Duration::seconds(1));
    assert_eq!(
        plan_expired_reservation(&cleanup, now, grace),
        Some(ExpiredReservationPlan::DeleteCleanup)
    );
    let active = file_reservation(FILE_UPLOAD_STATUS_PENDING, now + Duration::seconds(1));
    assert_eq!(plan_expired_reservation(&active, now, grace), None);
    let mut deleted = cleanup;
    deleted.del_flag = "1".to_owned();
    assert_eq!(plan_expired_reservation(&deleted, now, grace), None);
}

#[test]
fn route_key_is_trimmed_and_blank_becomes_absent() {
    assert_eq!(
        normalize_route_key(Some(" user.list ".into())).as_deref(),
        Some("user.list")
    );
    assert_eq!(normalize_route_key(Some("  ".into())), None);
}

#[test]
fn localized_message_requires_identical_arguments() {
    let mut title_args = BTreeMap::new();
    title_args.insert("name".into(), "A".into());
    let mut body_args = BTreeMap::new();
    body_args.insert("name".into(), "B".into());
    assert!(
        validate_message_text_pair(
            MessageText::Key {
                key: "title".into(),
                args: title_args,
            },
            MessageText::Key {
                key: "body".into(),
                args: body_args,
            },
        )
        .is_err()
    );
}

#[test]
fn tenant_permission_boundary_is_case_insensitive_and_fail_closed() {
    assert!(ensure_tenant_permission_code_boundary("system", "tenant:read").is_ok());
    assert!(ensure_tenant_permission_code_boundary("demo", "TENANT:read").is_err());
    assert!(is_platform_permission_code("PLATFORM:ops"));
}

#[test]
fn product_version_rejects_duplicate_capabilities() {
    let capability = || ProductCapabilityRecord {
        code: SERVICE_ACCOUNTS_CAPABILITY.into(),
        variant: "default".into(),
        schema_version: 1,
        config: serde_json::json!({}),
    };
    let version = |capabilities| ProductVersionRecord {
        id: 1,
        version: 1,
        name: "基础版".into(),
        description: None,
        status: "draft".into(),
        created_by: 2,
        published_by: None,
        published_at: None,
        capabilities,
    };
    assert!(ProductService::version_record_vo(version(vec![capability()])).is_ok());
    assert!(ProductService::version_record_vo(version(vec![capability(), capability()])).is_err());
}

#[test]
fn preferred_locale_accepts_only_supported_values() {
    assert_eq!(normalize_preferred_locale(None).unwrap(), None);
    assert_eq!(normalize_preferred_locale(Some("  ".into())).unwrap(), None);
    assert_eq!(
        normalize_preferred_locale(Some("zh-CN".into())).unwrap(),
        Some("zh-CN".into())
    );
    assert!(normalize_preferred_locale(Some("zh-cn".into())).is_err());
}

#[test]
fn delegated_capability_requires_both_permission_sets() {
    let capabilities = vec![
        ServiceCapabilityDescriptor {
            key: "read".to_owned(),
            permission: "system:user:list".to_owned(),
            direct: true,
            delegated: true,
        },
        ServiceCapabilityDescriptor {
            key: "write".to_owned(),
            permission: "system:user:create".to_owned(),
            direct: true,
            delegated: false,
        },
    ];
    let permissions = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<HashSet<_>>()
    };
    let common = common_capabilities(
        &capabilities,
        &permissions(&["system:user:*"]),
        &permissions(&["system:user:list"]),
    );
    assert_eq!(common.len(), 1);
    assert_eq!(common[0].key, "read");
}

#[test]
fn tenant_data_digest_is_stable_and_order_sensitive() {
    let first = vec![vec![Some("a".into()), None], vec![Some("b".into())]];
    let second = vec![vec![Some("b".into())], vec![Some("a".into()), None]];
    assert_eq!(
        rolling_digest(None, &first).expect("应计算摘要"),
        rolling_digest(None, &first).expect("相同行应得到相同摘要")
    );
    assert_ne!(
        rolling_digest(None, &first).expect("应计算摘要"),
        rolling_digest(None, &second).expect("应计算摘要")
    );
}

#[test]
fn tenant_quota_and_expiration_boundaries_are_stable() {
    assert_eq!(quota_status(1, 0), "unlimited");
    assert_eq!(quota_status(79, 100), "normal");
    assert_eq!(quota_status(80, 100), "warning");
    assert_eq!(quota_status(90, 100), "critical");
    assert_eq!(quota_status(100, 100), "exceeded");
    assert_eq!(percentage_basis_points(1, 3), Some(3333));
    let now = Utc::now();
    assert_eq!(expiration_status(&tenant(None), now), "never");
    assert_eq!(expiration_status(&tenant(Some(now)), now), "expired");
    assert_eq!(
        expiration_status(&tenant(Some(now + Duration::days(30))), now),
        "expiring"
    );
    assert_eq!(
        expiration_status(&tenant(Some(now + Duration::days(31))), now),
        "active"
    );
}

#[test]
fn department_paths_follow_hierarchy_and_enabled_state() {
    let paths = available_department_paths(
        vec![
            department(1, "总部", None, "0", "1"),
            department(2, "研发部", Some(1), "0,1", "1"),
        ],
        &actor(),
    )
    .unwrap();
    assert_eq!(paths, ["总部", "总部 / 研发部"]);
    let disabled = available_department_paths(
        vec![
            department(1, "总部", None, "0", "0"),
            department(2, "研发部", Some(1), "0,1", "1"),
        ],
        &actor(),
    )
    .unwrap();
    assert!(disabled.is_empty());
}

#[test]
fn import_source_state_bucket_and_digest_fail_closed() {
    assert!(
        !validate_import_source(
            &import_source(UserImportSourceState::Ready),
            &"a".repeat(64)
        )
        .unwrap()
    );
    assert!(
        validate_import_source(
            &import_source(UserImportSourceState::Recoverable),
            &"a".repeat(64)
        )
        .unwrap()
    );
    assert!(
        validate_import_source(
            &import_source(UserImportSourceState::Unavailable),
            &"a".repeat(64)
        )
        .is_err()
    );
    let mut candidate = import_source(UserImportSourceState::Ready);
    candidate.bucket = "uploads".into();
    assert!(validate_import_source(&candidate, &"a".repeat(64)).is_err());
    candidate.bucket = "imports".into();
    assert!(validate_import_source(&candidate, &"b".repeat(64)).is_err());
}

#[test]
fn user_command_inputs_are_normalized_and_validated() {
    let mut ids = vec![9, 2, 9, 5];
    normalize_ids(&mut ids);
    assert_eq!(ids, vec![2, 5, 9]);
    assert!(validate_manageable_status(USER_STATUS_NORMAL).is_ok());
    assert!(validate_manageable_status(USER_STATUS_DISABLED).is_ok());
    assert!(validate_manageable_status(USER_STATUS_PENDING_ACTIVATION).is_err());
}

#[test]
fn password_reset_state_is_strict() {
    assert!(ensure_pending(PASSWORD_RESET_STATUS_PENDING, None).is_ok());
    assert!(ensure_pending("completed", None).is_err());
    assert!(
        ensure_pending(
            PASSWORD_RESET_STATUS_PENDING,
            Some(chrono::DateTime::<Utc>::UNIX_EPOCH)
        )
        .is_err()
    );
    assert_eq!(
        password_reset_next_status(USER_STATUS_PENDING_ACTIVATION),
        USER_STATUS_NORMAL
    );
    assert_eq!(
        password_reset_next_status(USER_STATUS_MUST_RESET_PASSWORD),
        USER_STATUS_NORMAL
    );
    assert_eq!(password_reset_next_status("1"), "1");
    let regular = PasswordResetUserState {
        id: 7,
        authorization_version: 2,
        status: USER_STATUS_NORMAL.to_owned(),
        has_super_role: false,
    };
    assert!(ensure_not_super(&regular).is_ok());
    assert!(
        ensure_not_super(&PasswordResetUserState {
            has_super_role: true,
            ..regular
        })
        .is_err()
    );
}

#[test]
fn user_assignment_validation_fails_closed() {
    assert!(
        validate_assignment_state(
            assignment_state(vec![UserAssignmentRole {
                status_normal: true,
                is_super: false
            }]),
            None,
            Some(&[1]),
            false,
        )
        .is_ok()
    );
    assert!(
        validate_assignment_state(assignment_state(Vec::new()), None, Some(&[1]), false).is_err()
    );
    assert!(
        validate_assignment_state(
            assignment_state(vec![UserAssignmentRole {
                status_normal: false,
                is_super: false
            }]),
            None,
            Some(&[1]),
            false,
        )
        .is_err()
    );
    assert!(
        validate_assignment_state(
            assignment_state(vec![UserAssignmentRole {
                status_normal: true,
                is_super: true
            }]),
            None,
            Some(&[1]),
            false,
        )
        .is_err()
    );
}
