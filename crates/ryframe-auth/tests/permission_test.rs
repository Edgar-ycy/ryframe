use ryframe_auth::{RequestPrincipal, permission::check_permission};
use ryframe_common::{ActorContext, annotations::data_scope::DataScope};

fn principal(permissions: Vec<String>, is_super_admin: bool) -> RequestPrincipal {
    RequestPrincipal {
        actor: ActorContext {
            user_id: 1,
            tenant_id: "system".into(),
            username: "tester".into(),
            dept_id: None,
            dept_path: None,
            data_scope: DataScope::SelfOnly,
            custom_dept_ids: Vec::new(),
            include_self: true,
            is_super_admin,
        },
        roles: vec!["operator".into()],
        role_ids: vec![1],
        permissions,
        tenant_request_limit_per_minute: 100,
    }
}

#[test]
fn test_check_permission() {
    let principal = principal(
        vec!["system:user:list".into(), "system:user:*".into()],
        false,
    );
    assert!(check_permission(&principal, "system:user:list").is_ok());
    assert!(check_permission(&principal, "system:role:list").is_err());
    assert!(check_permission(&principal, "system:user:create").is_ok());
}

#[test]
fn test_super_admin_bypasses_permission_check() {
    let principal = principal(Vec::new(), true);
    assert!(check_permission(&principal, "system:any:action").is_ok());
}

#[test]
fn blank_required_permission_fails_closed_for_regular_users() {
    let regular_user = principal(Vec::new(), false);
    assert!(check_permission(&regular_user, "").is_err());
    assert!(check_permission(&regular_user, "   ").is_err());

    let super_admin = principal(Vec::new(), true);
    assert!(check_permission(&super_admin, "").is_err());
}
