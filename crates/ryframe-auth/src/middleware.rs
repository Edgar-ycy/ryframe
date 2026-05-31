use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ryframe_common::AppError;
use ryframe_config::AppConfig;
use ryframe_core::TokenBlacklist;

use crate::{
    jwt::{Claims, decode_token},
    permission::check_permission,
};

/// 认证中间件状态（合并 Config + TokenBlacklist）
#[derive(Clone)]
pub struct AuthState {
    pub config: Arc<AppConfig>,
    pub blacklist: TokenBlacklist,
}

/// 认证中间件
///
/// 从 Authorization 头提取 Bearer token，验证 JWT 签名和有效期，
/// 检查 Token 黑名单（支持 JWT 主动撤销），并将 Claims 注入到 extensions。
/// 需要在 Router 上注册：
/// ```ignore
/// Router::new().route_layer(middleware::from_fn_with_state(auth_state, auth_middleware))
/// ```
pub async fn auth_middleware(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Result<Response, Response> {
    let token = match extract_bearer_token(&request) {
        Some(t) => t,
        None => return Err(AppError::Authentication("缺少认证令牌".into()).into_response()),
    };

    let claims = match decode_token(&token, &auth_state.config.auth.jwt_secret) {
        Ok(c) => c,
        Err(e) => return Err(e.into_response()),
    };

    if claims.token_type != "access" {
        return Err(
            AppError::Authentication("令牌类型错误，请使用访问令牌".into()).into_response(),
        );
    }

    // Token 黑名单检查（支持 JWT 主动撤销）
    if auth_state.blacklist.is_blacklisted(&claims.jti).await {
        return Err(AppError::Authentication("令牌已被撤销，请重新登录".into()).into_response());
    }

    request.extensions_mut().insert(claims);
    Ok(next.run(request).await)
}

/// 从请求头提取 Bearer token
fn extract_bearer_token(request: &Request) -> Option<String> {
    let header = request.headers().get("Authorization")?.to_str().ok()?;
    header.strip_prefix("Bearer ").map(|s| s.to_string())
}

/// 权限守卫中间件工厂
///
/// 使用方式（路由级）：
/// ```ignore
/// .route("/users", get(list_users).route_layer(middleware::from_fn_with_state(
///     auth_state.clone(),
///     require_permission("system:user:list"),
/// )))
/// ```
#[allow(clippy::type_complexity)]
pub fn require_permission(
    perm: &'static str,
) -> impl Fn(
    State<AuthState>,
    Request,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, Response>> + Send>>
+ Clone {
    move |_state: State<AuthState>, request: Request, next: Next| {
        let perm = perm;
        Box::pin(async move {
            let claims = request.extensions().get::<Claims>().ok_or_else(|| {
                AppError::Authentication("未认证，请先登录".into()).into_response()
            })?;

            check_permission(claims, perm).map_err(|e| e.into_response())?;

            Ok(next.run(request).await)
        })
    }
}
