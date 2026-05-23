use crate::{jwt::Claims, rbac};
use ryframe_common::{AppError, AppResult};

/// 从 Claims 校验用户是否拥有指定权限
///
/// 不满足权限时返回 `AppError::Authorization("权限不足")`。
pub fn check_permission(claims: &Claims, required: &str) -> AppResult<()> {
    if !rbac::has_permission(&claims.perms, required) {
        return Err(AppError::Authorization(format!(
            "权限不足，需要权限码: {}",
            required
        )));
    }
    Ok(())
}

/// 从 Claims 校验用户是否拥有指定角色
pub fn check_role(claims: &Claims, required: &str) -> AppResult<()> {
    if !rbac::has_role(&claims.roles, required) {
        return Err(AppError::Authorization(format!(
            "角色不满足，需要角色: {}",
            required
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_claims(perms: Vec<&str>, roles: Vec<&str>) -> Claims {
        Claims {
            sub: "1".to_string(),
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
}