use ryframe_auth::permission::{PermissionContext, check_permission_context};

#[test]
fn test_check_permission() {
    let context = PermissionContext {
        roles: vec!["operator".into()],
        permissions: vec!["system:user:list".into(), "system:user:*".into()],
        is_super_admin: false,
    };
    // 精确匹配
    assert!(check_permission_context(&context, "system:user:list").is_ok());
    // 不同模块无权限
    assert!(check_permission_context(&context, "system:role:list").is_err());
    // 通配符
    assert!(check_permission_context(&context, "system:user:create").is_ok());
}

#[test]
fn test_super_admin_bypasses_permission_check() {
    let context = PermissionContext {
        roles: vec!["admin".into()],
        permissions: vec![],
        is_super_admin: true,
    };
    assert!(check_permission_context(&context, "system:any:action").is_ok());
}
