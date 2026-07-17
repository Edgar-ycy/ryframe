use ryframe_common::{AppError, AppResult};

use crate::{principal::RequestPrincipal, rbac};

pub fn check_permission(principal: &RequestPrincipal, required: &str) -> AppResult<()> {
    if principal.is_super_admin || rbac::has_permission(&principal.permissions, required) {
        return Ok(());
    }
    Err(AppError::Authorization(format!(
        "权限不足，需要权限码: {required}"
    )))
}
