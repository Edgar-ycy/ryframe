use chrono::{TimeZone, Utc};
use ryframe_application::{
    JobWorkerPolicy, PersistedTraceContext, current_trace_context, has_super_admin_role,
    install_id_generator, is_valid_tenant_target_key, next_id, validate_cache_namespace,
    validate_persisted_schedule_configuration,
};
use ryframe_application::{
    ports::auth::IdentityRoleRecord,
    system::{RoleOptionPurpose, parse_log_time_range},
};
use ryframe_kernel::{AppError, AppResult};

fn fixed_id() -> AppResult<i64> {
    Ok(43)
}

fn role(code: &str, is_super: bool) -> IdentityRoleRecord {
    IdentityRoleRecord {
        id: 1,
        code: code.into(),
        is_super,
        data_scope: "5".into(),
    }
}

#[test]
fn missing_trace_adapter_fails_closed() {
    assert_eq!(current_trace_context(), PersistedTraceContext::default());
}

#[test]
fn tenant_target_key_validation_is_strict() {
    assert!(is_valid_tenant_target_key("shared-control"));
    assert!(!is_valid_tenant_target_key("-invalid"));
    assert!(!is_valid_tenant_target_key("invalid value"));
}

#[test]
fn worker_policy_rejects_heartbeat_outside_lease() {
    assert!(JobWorkerPolicy::new(None, 60, 60, 500, 5_000, 15, 4).is_err());
}

#[test]
fn installed_generator_is_used_and_cannot_be_replaced() {
    install_id_generator(fixed_id).expect("首次安装应成功");
    assert_eq!(next_id().expect("ID 应生成成功"), 43);
    assert!(install_id_generator(fixed_id).is_err());
}

#[test]
fn super_admin_uses_explicit_role_marker_instead_of_code() {
    assert!(!has_super_admin_role(&[role("admin", false)]));
    assert!(has_super_admin_role(&[role("ordinary", true)]));
}

#[test]
fn cache_namespace_rejects_unsafe_fragments() {
    for value in ["config", "tenant.cache-v1", "system.config-items_v2"] {
        assert!(validate_cache_namespace(value).is_ok());
    }
    for value in ["", "Config", "../config", "system/cache", &"a".repeat(65)] {
        assert!(validate_cache_namespace(value).is_err());
    }
}

#[test]
fn persisted_schedule_validation_uses_application_values() {
    let now = Utc.with_ymd_and_hms(2026, 8, 21, 0, 0, 0).unwrap();
    let next_run_at = Utc.with_ymd_and_hms(2026, 8, 22, 0, 0, 0).unwrap();
    let valid = validate_persisted_schedule_configuration(
        Some(next_run_at),
        "skip",
        "forbid",
        300,
        "0 0 0 * * * *",
        "Asia/Shanghai",
        now,
    );
    assert!(valid.is_ok());

    let invalid = validate_persisted_schedule_configuration(
        Some(next_run_at),
        "unknown",
        "forbid",
        300,
        "0 0 0 * * * *",
        "Asia/Shanghai",
        now,
    );
    assert_eq!(
        invalid.err().as_deref(),
        Some("错过执行策略只能是 skip 或 fire_once")
    );
}

#[test]
fn log_time_range_is_strict_and_normalized() {
    let (begin, end) = parse_log_time_range(
        Some(" 2026-08-20T10:00:00+08:00 "),
        Some("2026-08-20T03:00:00Z"),
    )
    .expect("有效时间区间应通过");
    assert_eq!(
        begin.map(|time| time.to_rfc3339()),
        Some("2026-08-20T02:00:00+00:00".into())
    );
    assert_eq!(
        end.map(|time| time.to_rfc3339()),
        Some("2026-08-20T03:00:00+00:00".into())
    );
    assert_eq!(
        parse_log_time_range(Some("  "), None).expect("空值应转为缺省"),
        (None, None)
    );
    assert!(matches!(
        parse_log_time_range(Some("2026-08-20"), None),
        Err(AppError::Validation(_))
    ));
    assert!(matches!(
        parse_log_time_range(Some("2026-08-20T04:00:00Z"), Some("2026-08-20T03:00:00Z")),
        Err(AppError::Validation(_))
    ));
}

#[test]
fn role_option_purpose_controls_super_role_visibility() {
    assert!(RoleOptionPurpose::UserAssignment.includes_super_role(true));
    assert!(!RoleOptionPurpose::UserAssignment.includes_super_role(false));
    assert!(!RoleOptionPurpose::ServiceAccountAssignment.includes_super_role(true));
    assert!(!RoleOptionPurpose::ServiceAccountAssignment.includes_super_role(false));
}
