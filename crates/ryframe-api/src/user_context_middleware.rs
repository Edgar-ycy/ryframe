use axum::{
    extract::{Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ryframe_auth::jwt::Claims;
use ryframe_common::{AppError, annotations::data_scope::DataScopeContext};
use ryframe_core::TenantContext;

use crate::{extractors::CurrentUser, handlers::auth_handler::AppState};

/// 用户上下文中间件
///
/// 在 auth_middleware 之后运行（Claims 已在 extensions 中）。
/// 查询用户的数据权限上下文，构建 CurrentUser 并注入到 request extensions。
///
/// 执行顺序（从外到内）：auth_middleware → user_context_middleware → handler
pub async fn user_context_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, Response> {
    let claims = request
        .extensions()
        .get::<Claims>()
        .cloned()
        .ok_or_else(|| AppError::Authentication("未认证，请先登录".into()).into_response())?;

    let user_id: i64 = claims
        .sub
        .parse()
        .map_err(|_| AppError::Authentication("令牌中的用户ID无效".into()).into_response())?;

    let is_super_admin =
        claims.roles.contains(&"admin".to_string()) || claims.perms.contains(&"*:*:*".to_string());
    let tenant_id = request
        .extensions()
        .get::<TenantContext>()
        .map(|ctx| ctx.tenant_id.clone())
        .unwrap_or_else(|| "system".to_string());

    // 构建数据权限上下文（复用 UserServiceImpl 已有逻辑）
    let (scope_ctx, role_ids) = if is_super_admin {
        (DataScopeContext::super_admin(user_id), vec![])
    } else {
        match build_scope_ctx(&state, user_id).await {
            Ok(result) => result,
            Err(e) => return Err(e.into_response()),
        }
    };

    let current_user = CurrentUser {
        user_id,
        tenant_id,
        username: claims.username.clone(),
        roles: claims.roles.clone(),
        role_ids,
        permissions: claims.perms.clone(),
        dept_id: scope_ctx.dept_id,
        dept_path: scope_ctx.ancestors.clone(),
        data_scope: scope_ctx.scope.clone(),
        custom_dept_ids: scope_ctx.custom_dept_ids.clone(),
        is_super_admin,
    };

    request.extensions_mut().insert(current_user);
    Ok(next.run(request).await)
}

/// 构建 DataScopeContext（复用 UserServiceImpl 已有逻辑），同时返回角色ID列表
async fn build_scope_ctx(
    state: &AppState,
    user_id: i64,
) -> Result<(DataScopeContext, Vec<i64>), AppError> {
    // 查用户获取 dept_id
    let user_vo = state
        .user_service
        .find_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::Authentication("用户不存在".into()))?;
    if user_vo.status != ryframe_db::entities::user::Model::STATUS_NORMAL {
        return Err(AppError::Authentication("账号已停用或锁定".into()));
    }

    // 查用户角色（通过 role_repo 获取 Model，含 data_scope 值）
    let roles = state
        .user_service
        .role_repo
        .find_user_roles(&state.db, user_id)
        .await?;

    let role_ids: Vec<i64> = roles.iter().map(|r| r.id).collect();

    // 复用 UserServiceImpl 的 build_data_scope_context
    let ctx = state
        .user_service
        .build_data_scope_context(&state.db, user_id, user_vo.dept_id, &roles)
        .await?;

    Ok((ctx, role_ids))
}
