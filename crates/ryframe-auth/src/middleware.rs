use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
};

use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};
use ryframe_adapters::{RefreshSessionStore, TenantContext, TokenBlacklist, with_tenant_context};
use ryframe_config::AppConfig;
use ryframe_http::HttpAppError;
use ryframe_kernel::AppError;

use crate::{
    jwt::decode_token,
    permission::check_permission,
    principal::{PrincipalResolver, RequestPrincipal},
};

static BACKEND_FAILURE_HOOK: OnceLock<fn(&str)> = OnceLock::new();

/// 安装进程级观测钩子，且不会引入 auth 到 middleware 的依赖环。重复安装无害。
pub fn set_backend_failure_hook(hook: fn(&str)) {
    let _ = BACKEND_FAILURE_HOOK.set(hook);
}

fn record_backend_failure(subsystem: &str) {
    if let Some(hook) = BACKEND_FAILURE_HOOK.get() {
        hook(subsystem);
    }
}

/// 认证中间件状态（合并配置与令牌黑名单）
#[derive(Clone)]
pub struct AuthState {
    pub config: Arc<AppConfig>,
    pub blacklist: TokenBlacklist,
    pub principal_resolver: Arc<dyn PrincipalResolver>,
    pub refresh_sessions: RefreshSessionStore,
}

/// 认证中间件
///
/// 从 `Authorization` 请求头提取 `Bearer` 令牌，验证 JWT 签名和有效期，
/// 检查令牌黑名单（支持 JWT 主动撤销），并将 JWT 声明注入请求扩展。
/// 需要在 Router 上注册：
/// ```text
/// # use ryframe_auth::middleware::auth_middleware;
/// // Router::new().route_layer(middleware::from_fn_with_state(auth_state, auth_middleware))
/// ```
pub async fn auth_middleware(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Result<Response, Response> {
    let token = match extract_bearer_token(&request) {
        Some(t) => t,
        None => {
            return Err(
                HttpAppError::from(AppError::Authentication("缺少认证令牌".into())).into_response(),
            );
        }
    };

    let claims = match decode_token(&token, &auth_state.config.auth.jwt_secret) {
        Ok(c) => c,
        Err(error) => return Err(HttpAppError::from(error).into_response()),
    };

    if claims.token_type != "access" {
        return Err(HttpAppError::from(AppError::Authentication(
            "令牌类型错误，请使用访问令牌".into(),
        ))
        .into_response());
    }

    if !auth_state
        .config
        .multi_tenancy
        .allows_tenant(&claims.tenant_id)
    {
        return Err(HttpAppError::from(AppError::Authentication(
            "令牌租户不适用于当前运行模式，请重新登录".into(),
        ))
        .into_response());
    }

    // 令牌黑名单检查（支持 JWT 主动撤销）
    if auth_state
        .blacklist
        .try_is_blacklisted(&claims.jti)
        .await
        .map_err(|error| {
            record_backend_failure("access_revocation");
            HttpAppError::from(error).into_response()
        })?
    {
        return Err(HttpAppError::from(AppError::Authentication(
            "令牌已被撤销，请重新登录".into(),
        ))
        .into_response());
    }

    let claims_user_id = claims.sub.parse::<i64>().map_err(|_| {
        HttpAppError::from(AppError::Authentication("令牌主体无效".into())).into_response()
    })?;
    if !auth_state
        .refresh_sessions
        .is_active_for_identity(&claims.sid, &claims.tenant_id, claims_user_id)
        .await
        .map_err(|error| {
            record_backend_failure("access_session");
            HttpAppError::from(error).into_response()
        })?
    {
        return Err(HttpAppError::from(AppError::Authentication(
            "session is no longer active".into(),
        ))
        .into_response());
    }

    // 用已验证令牌中绑定的租户身份替换未认证、由请求头派生的上下文。
    let tenant_context = TenantContext {
        tenant_id: claims.tenant_id.clone(),
        is_admin: false,
    };
    let principal = with_tenant_context(
        tenant_context.clone(),
        auth_state.principal_resolver.resolve_principal(&claims),
    )
    .await
    .map_err(|error| HttpAppError::from(error).into_response())?;

    let span = tracing::Span::current();
    span.record("tenant.id", principal.tenant_id.as_str());
    span.record("user.id", principal.user_id);
    span.record("user.name", principal.username.as_str());

    let authorization_epoch = principal.tenant_authorization_epoch;
    request.extensions_mut().insert(tenant_context.clone());
    request.extensions_mut().insert(principal);
    request.extensions_mut().insert(claims);
    let mut response = with_tenant_context(tenant_context, next.run(request)).await;
    if let Ok(value) = HeaderValue::from_str(&authorization_epoch.to_string()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-authorization-epoch"), value);
    }
    Ok(response)
}

/// 从请求头提取 `Bearer` 令牌
fn extract_bearer_token(request: &Request) -> Option<String> {
    let header = request.headers().get("Authorization")?.to_str().ok()?;
    header.strip_prefix("Bearer ").map(|s| s.to_string())
}

type PermissionFuture = Pin<Box<dyn Future<Output = Result<Response, Response>> + Send>>;

/// 权限守卫中间件工厂
///
/// 使用方式（路由级，无需状态）：
/// ```text
/// # use ryframe_auth::middleware::require_permission;
/// // .route("/users", get(list_users).route_layer(middleware::from_fn(
/// //     require_permission("system:user:list"),
/// // )))
/// ```
pub fn require_permission(
    perm: &'static str,
) -> impl Fn(Request, Next) -> PermissionFuture + Clone {
    move |request: Request, next: Next| {
        let perm = perm;
        Box::pin(async move {
            let context = request
                .extensions()
                .get::<RequestPrincipal>()
                .ok_or_else(|| {
                    HttpAppError::from(AppError::Authentication("未认证，请先登录".into()))
                        .into_response()
                })?;

            check_permission(context, perm)
                .map_err(|error| HttpAppError::from(error).into_response())?;

            Ok(next.run(request).await)
        })
    }
}

pub fn perm_route<S>(route: MethodRouter<S>, perm: &'static str) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    route.route_layer(middleware::from_fn(require_permission(perm)))
}
