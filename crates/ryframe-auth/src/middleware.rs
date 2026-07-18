use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
};

use axum::{
    extract::{Request, State},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};
use ryframe_common::AppError;
use ryframe_config::AppConfig;
use ryframe_core::{RefreshSessionStore, TenantContext, TokenBlacklist, with_tenant_context};

use crate::{
    jwt::decode_token,
    permission::check_permission,
    principal::{PrincipalResolver, RequestPrincipal},
};

static BACKEND_FAILURE_HOOK: OnceLock<fn(&str)> = OnceLock::new();

/// Install a process-wide observer without introducing an auth -> middleware
/// dependency cycle. Repeated installation is harmless.
pub fn set_backend_failure_hook(hook: fn(&str)) {
    let _ = BACKEND_FAILURE_HOOK.set(hook);
}

fn record_backend_failure(subsystem: &str) {
    if let Some(hook) = BACKEND_FAILURE_HOOK.get() {
        hook(subsystem);
    }
}

/// 认证中间件状态（合并 Config + TokenBlacklist）
#[derive(Clone)]
pub struct AuthState {
    pub config: Arc<AppConfig>,
    pub blacklist: TokenBlacklist,
    pub principal_resolver: Arc<dyn PrincipalResolver>,
    pub refresh_sessions: RefreshSessionStore,
}

/// 认证中间件
///
/// 从 Authorization 头提取 Bearer token，验证 JWT 签名和有效期，
/// 检查 Token 黑名单（支持 JWT 主动撤销），并将 Claims 注入到 extensions。
/// 需要在 Router 上注册：
/// ```
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
    if auth_state
        .blacklist
        .try_is_blacklisted(&claims.jti)
        .await
        .map_err(|error| {
            record_backend_failure("access_revocation");
            error.into_response()
        })?
    {
        return Err(AppError::Authentication("令牌已被撤销，请重新登录".into()).into_response());
    }

    if claims.sid.is_empty() {
        return Err(
            AppError::Authentication("legacy access token is not accepted".into()).into_response(),
        );
    }
    if !auth_state
        .refresh_sessions
        .is_active(&claims.sid)
        .await
        .map_err(|error| {
            record_backend_failure("access_session");
            error.into_response()
        })?
    {
        return Err(AppError::Authentication("session is no longer active".into()).into_response());
    }

    // Replace the unauthenticated, header-derived context with the tenant
    // identity bound in the verified token.
    let tenant_context = TenantContext {
        tenant_id: claims.tenant_id.clone(),
        is_admin: false,
    };
    let principal = with_tenant_context(
        tenant_context.clone(),
        auth_state.principal_resolver.resolve_principal(&claims),
    )
    .await
    .map_err(|error| error.into_response())?;

    let span = tracing::Span::current();
    span.record("tenant.id", principal.tenant_id.as_str());
    span.record("user.id", principal.user_id);
    span.record("user.name", principal.username.as_str());

    request.extensions_mut().insert(tenant_context.clone());
    request.extensions_mut().insert(principal);
    request.extensions_mut().insert(claims);
    Ok(with_tenant_context(tenant_context, next.run(request)).await)
}

/// 从请求头提取 Bearer token
fn extract_bearer_token(request: &Request) -> Option<String> {
    let header = request.headers().get("Authorization")?.to_str().ok()?;
    header.strip_prefix("Bearer ").map(|s| s.to_string())
}

type PermissionFuture = Pin<Box<dyn Future<Output = Result<Response, Response>> + Send>>;

/// 权限守卫中间件工厂
///
/// 使用方式（路由级，无需 State）：
/// ```
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
                    AppError::Authentication("未认证，请先登录".into()).into_response()
                })?;

            check_permission(context, perm).map_err(|e| e.into_response())?;

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
