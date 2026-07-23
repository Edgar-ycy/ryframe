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
    // Public routes must omit the permission middleware. Failing closed here
    // prevents a blank route annotation from silently disabling authorization.
    if required.trim().is_empty() {
        return false;
    }

    user_perms
        .iter()
        .any(|p| p == required || p == "*:*:*" || wildcard_match(p, required))
}

fn wildcard_match(pattern: &str, required: &str) -> bool {
    if pattern == "*:*:*" {
        return true;
    }
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
    if required.trim().is_empty() {
        return false;
    }
    user_roles.iter().any(|r| r == required)
}
