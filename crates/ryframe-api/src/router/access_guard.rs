use axum::{
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
};
use ryframe_application::system::platform::{is_platform_permission, is_platform_route};
use ryframe_kernel::AppError;

use crate::{
    RequestPrincipal,
    http::{HttpAppError, HttpResult},
    state::AppState,
};

/// 对编译目录中的管理路由统一执行平台和能力门禁，权限通配符不能绕过。
pub(super) async fn catalog_access_guard(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> HttpResult<Response> {
    let principal = request
        .extensions()
        .get::<RequestPrincipal>()
        .ok_or_else(|| AppError::Authentication("未认证，请先登录".into()))?;
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .ok_or_else(|| AppError::Internal("已认证路由缺少编译路径".into()))?
        .as_str();
    let policy = crate::permission_catalog::route_policies()
        .iter()
        .find(|policy| policy.method == request.method().as_str() && policy.path == path);
    let platform_only = path.starts_with("/api/v1/platform/")
        || policy
            .and_then(|value| value.permission_code)
            .is_some_and(is_platform_permission);
    if platform_only && principal.tenant_id != "system" {
        return Err(HttpAppError::from(AppError::Authorization(
            "仅系统租户可以操作平台控制功能".into(),
        )));
    }
    if let Some(code) = policy.and_then(|value| value.capability_code) {
        state
            .services
            .platform
            .product
            .require_capability(&principal.tenant_id, code)
            .await?;
    }
    Ok(next.run(request).await)
}

pub(crate) fn excluded_platform_routes(
    tenant_id: &str,
    multi_tenancy_enabled: bool,
) -> Vec<String> {
    crate::permission_catalog::menu_routes()
        .iter()
        .filter(|menu| {
            (tenant_id != "system" && is_platform_route(menu.route_key))
                || (!multi_tenancy_enabled
                    && matches!(menu.route_key, "platform.tenant" | "platform.product-plans"))
        })
        .map(|menu| menu.route_key.to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_tenant_keeps_data_and_configuration_tools() {
        let excluded = excluded_platform_routes("system", false);
        assert!(excluded.iter().any(|key| key == "platform.tenant"));
        assert!(excluded.iter().any(|key| key == "platform.product-plans"));
        assert!(!excluded.iter().any(|key| key == "platform.config-transfer"));
        assert!(!excluded.iter().any(|key| key == "platform.data-targets"));
        assert!(excluded_platform_routes("system", true).is_empty());
        assert!(
            excluded_platform_routes("other", true)
                .iter()
                .any(|key| key == "platform.data-targets")
        );
    }
}
