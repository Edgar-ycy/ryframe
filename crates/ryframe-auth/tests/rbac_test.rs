use ryframe_auth::rbac::{has_permission, has_role};

#[test]
fn test_has_permission() {
    // 精确匹配
    let perms = vec!["system:user:list".to_string()];
    assert!(has_permission(&perms, "system:user:list"));
    assert!(!has_permission(&perms, "system:user:create"));

    // 通配符
    let perms = vec!["system:user:*".to_string()];
    assert!(has_permission(&perms, "system:user:list"));
    assert!(!has_permission(&perms, "system:role:list"));
    assert!(!has_permission(&perms, "system:user2:list"));

    // 超管
    let perms = vec!["*:*:*".to_string()];
    assert!(has_permission(&perms, "anything:at:all"));
    assert!(has_permission(&perms, "tenant:manage"));

    // 空权限
    assert!(!has_permission(&Vec::<String>::new(), ""));
    assert!(!has_permission(&Vec::<String>::new(), "   "));

    // `admin` is a role code, not a magic permission. Super-admin bypass is
    // represented explicitly on RequestPrincipal; `*:*:*` remains the only
    // persisted all-permissions code.
    let perms = vec!["admin".to_string()];
    assert!(!has_permission(&perms, "system:user:list"));
}

#[test]
fn test_has_role() {
    let roles = vec!["admin".to_string()];
    assert!(has_role(&roles, "admin"));
    assert!(!has_role(&roles, "user"));
    assert!(!has_role(&roles, ""));
    assert!(!has_role(&roles, "   "));
}
