use ryframe_common::{AppError, AppResult};

use crate::{jwt::Claims, rbac};

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
