use ryframe_auth::{
    jwt::Claims,
    permission::{check_permission, check_role},
};

fn make_claims(perms: Vec<&str>, roles: Vec<&str>) -> Claims {
    Claims {
        sub: "1".to_string(),
        tenant_id: "system".to_string(),
        tenant_session_version: 1,
        username: "test".to_string(),
        roles: roles.into_iter().map(|s| s.to_string()).collect(),
        perms: perms.into_iter().map(|s| s.to_string()).collect(),
        token_type: "access".to_string(),
        jti: "test-jti".to_string(),
        iat: 0,
        exp: 9999999999,
    }
}

#[test]
fn test_check_permission() {
    let claims = make_claims(vec!["system:user:list", "system:user:*"], vec![]);
    // 精确匹配
    assert!(check_permission(&claims, "system:user:list").is_ok());
    // 不同模块无权限
    assert!(check_permission(&claims, "system:role:list").is_err());
    // 通配符
    assert!(check_permission(&claims, "system:user:create").is_ok());
}

#[test]
fn test_check_role() {
    let claims = make_claims(vec![], vec!["admin"]);
    assert!(check_role(&claims, "admin").is_ok());
    assert!(check_role(&claims, "user").is_err());
}
