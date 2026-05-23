use crate::jwt::{Claims, decode_token};
use crate::permission::check_permission;
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ryframe_common::AppError;
use ryframe_config::AppConfig;
use std::sync::Arc;

/// 认证中间件
///
/// 从 Authorization 头提取 Bearer token，验证并注入 Claims 到 extensions。
/// 需要在 Router 上注册：
/// ```ignore
/// Router::new().route_layer(middleware::from_fn_with_state(config, auth_middleware))
/// ```
pub async fn auth_middleware(
    State(config): State<Arc<AppConfig>>,
    mut request: Request,
    next: Next,
) -> Result<Response, Response> {
    let token = match extract_bearer_token(&request) {
        Some(t) => t,
        None => return Err(AppError::Authentication("缺少认证令牌".into()).into_response()),
    };

    let claims = match decode_token(&token, &config.auth.jwt_secret) {
        Ok(c) => c,
        Err(e) => return Err(e.into_response()),
    };

    if claims.token_type != "access" {
        return Err(
            AppError::Authentication("令牌类型错误，请使用访问令牌".into()).into_response(),
        );
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
///     config.clone(),
///     require_permission("system:user:list"),
/// )))
/// ```
#[allow(clippy::type_complexity)]
pub fn require_permission(
    perm: &'static str,
) -> impl Fn(
    State<Arc<AppConfig>>,
    Request,
    Next,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, Response>> + Send>>
+ Clone {
    move |_state: State<Arc<AppConfig>>, request: Request, next: Next| {
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
