/// 检查用户是否拥有指定权限
///
/// 支持通配符匹配：
/// - `system:user:*` 匹配 `system:user:list`、`system:user:create` 等
/// - `system:*:*` 匹配 `system:user:list`、`system:role:list` 等
///
/// 示例：
/// ```ignore
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

    user_perms.iter().any(|p| {
        // 精确匹配
        if p == required {
            return true;
        }
        // 全局超级权限
        if p == "*:*:*" || p == "admin" {
            return true;
        }
        // 通配符匹配
        if p.ends_with(":*") {
            let prefix = &p[..p.len() - 2];
            return required.starts_with(prefix);
        }
        false
    })
}

/// 检查用户是否拥有指定角色
pub fn has_role(user_roles: &[String], required: &str) -> bool {
    if required.is_empty() {
        return true;
    }
    user_roles.iter().any(|r| r == required)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let perms = vec!["system:user:list".to_string()];
        assert!(has_permission(&perms, "system:user:list"));
        assert!(!has_permission(&perms, "system:user:create"));
    }

    #[test]
    fn test_wildcard_match() {
        let perms = vec!["system:user:*".to_string()];
        assert!(has_permission(&perms, "system:user:list"));
        assert!(has_permission(&perms, "system:user:create"));
        assert!(!has_permission(&perms, "system:role:list"));
    }

    #[test]
    fn test_super_admin() {
        let perms = vec!["*:*:*".to_string()];
        assert!(has_permission(&perms, "anything:at:all"));
    }

    #[test]
    fn test_empty_required() {
        let perms: Vec<String> = vec![];
        assert!(has_permission(&perms, ""));
    }

    #[test]
    fn test_has_role() {
        let roles = vec!["admin".to_string()];
        assert!(has_role(&roles, "admin"));
        assert!(!has_role(&roles, "user"));
    }
}