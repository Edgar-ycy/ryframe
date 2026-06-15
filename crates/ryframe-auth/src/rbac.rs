/// 检查用户是否拥有指定权限
///
/// 支持通配符匹配：
/// - `system:user:*` 匹配 `system:user:list`、`system:user:create` 等
/// - `system:*:*` 匹配 `system:user:list`、`system:role:list` 等
///
/// 示例：
/// ```
/// use ryframe_auth::rbac::has_permission;
///
/// let perms = vec!["system:user:*".to_string()];
/// assert!(has_permission(&perms, "system:user:list"));
/// assert!(has_permission(&perms, "system:user:create"));
/// assert!(!has_permission(&perms, "system:role:list"));
/// ```
pub fn has_permission(user_perms: &[String], required: &str) -> bool {
    // 空权限码表示公开接口，直接放行
    if required.is_empty() {
        return true;
    }

    user_perms
        .iter()
        .any(|p| p == required || p == "admin" || wildcard_match(p, required))
}

fn wildcard_match(pattern: &str, required: &str) -> bool {
    let pattern_parts: Vec<&str> = pattern.split(':').collect();
    let required_parts: Vec<&str> = required.split(':').collect();

    pattern_parts.len() == required_parts.len()
        && pattern_parts
            .iter()
            .zip(required_parts.iter())
            .all(|(pattern, required)| *pattern == "*" || pattern == required)
}

/// 检查用户是否拥有指定角色
pub fn has_role(user_roles: &[String], required: &str) -> bool {
    if required.is_empty() {
        return true;
    }
    user_roles.iter().any(|r| r == required)
}
