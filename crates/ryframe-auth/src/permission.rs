use ryframe_common::{AppError, AppResult};

use crate::{principal::RequestPrincipal, rbac};

pub fn check_permission(principal: &RequestPrincipal, required: &str) -> AppResult<()> {
    if required.trim().is_empty() {
        return Err(AppError::Authorization(
            "protected route is missing its permission code".into(),
        ));
    }
    if principal.is_super_admin || rbac::has_permission(&principal.permissions, required) {
        return Ok(());
    }
    Err(AppError::Authorization(format!(
        "权限不足，需要权限码: {required}"
    )))
}
