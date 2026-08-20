use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
};

use crate::http::HttpAppError;
use axum::{
    extract::{Request, State},
    http::{HeaderName, HeaderValue},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::MethodRouter,
};
use ryframe_adapters::{RefreshSessionStore, TokenBlacklist};
use ryframe_application::{PrincipalResolver, TenantContext, with_tenant_context};
use ryframe_auth::{
    RequestPrincipal as AuthPrincipal,
    jwt::{TokenSettings, decode_token},
    permission::check_permission,
};
use ryframe_kernel::AppError;

use crate::RequestPrincipal;

static BACKEND_FAILURE_HOOK: OnceLock<fn(&str)> = OnceLock::new();

/// 安装进程级认证依赖故障观测钩子。重复安装无害。
pub fn set_backend_failure_hook(hook: fn(&str)) {
    let _ = BACKEND_FAILURE_HOOK.set(hook);
}

fn record_backend_failure(subsystem: &str) {
    if let Some(hook) = BACKEND_FAILURE_HOOK.get() {
        hook(subsystem);
    }
}

/// HTTP 认证中间件状态。
#[derive(Clone)]
pub struct AuthState {
    pub token_settings: Arc<TokenSettings>,
    pub allow_multiple_tenants: bool,
    pub blacklist: TokenBlacklist,
    pub principal_resolver: Arc<dyn PrincipalResolver>,
    pub refresh_sessions: RefreshSessionStore,
}

pub async fn auth_middleware(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Result<Response, Response> {
    let token = extract_bearer_token(&request).ok_or_else(|| {
        HttpAppError::from(AppError::Authentication("缺少认证令牌".into())).into_response()
    })?;
    let claims = decode_token(token, &auth_state.token_settings)
        .map_err(|error| HttpAppError::from(error).into_response())?;

    if claims.token_type != "access" {
        return Err(HttpAppError::from(AppError::Authentication(
            "令牌类型错误，请使用访问令牌".into(),
        ))
        .into_response());
    }
    if !auth_state.allow_multiple_tenants && claims.tenant_id != "system" {
        return Err(HttpAppError::from(AppError::Authentication(
            "令牌租户不适用于当前运行模式，请重新登录".into(),
        ))
        .into_response());
    }

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
    let principal = Arc::new(principal);

    let span = tracing::Span::current();
    span.record("tenant.id", principal.tenant_id.as_str());
    span.record("user.id", principal.user_id);
    span.record("user.name", principal.username.as_str());

    let authorization_epoch = principal.tenant_authorization_epoch;
    request.extensions_mut().insert(tenant_context.clone());
    request
        .extensions_mut()
        .insert(Arc::<AuthPrincipal>::clone(&principal));
    request
        .extensions_mut()
        .insert(RequestPrincipal::new(principal));
    request.extensions_mut().insert(claims);
    let mut response = with_tenant_context(tenant_context, next.run(request)).await;
    if let Ok(value) = HeaderValue::from_str(&authorization_epoch.to_string()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-authorization-epoch"), value);
    }
    Ok(response)
}

fn extract_bearer_token(request: &Request) -> Option<&str> {
    request
        .headers()
        .get("Authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

type PermissionFuture = Pin<Box<dyn Future<Output = Result<Response, Response>> + Send>>;

pub fn require_permission(
    permission: &'static str,
) -> impl Fn(Request, Next) -> PermissionFuture + Clone {
    move |request: Request, next: Next| {
        Box::pin(async move {
            let principal = request
                .extensions()
                .get::<RequestPrincipal>()
                .ok_or_else(|| {
                    HttpAppError::from(AppError::Authentication("未认证，请先登录".into()))
                        .into_response()
                })?;
            check_permission(principal, permission)
                .map_err(|error| HttpAppError::from(error).into_response())?;
            Ok(next.run(request).await)
        })
    }
}

pub fn perm_route<S>(route: MethodRouter<S>, permission: &'static str) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    route.route_layer(middleware::from_fn(require_permission(permission)))
}
