use crate::RequestPrincipal;
use crate::http::{ApiResponse, HttpResult};
use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use ryframe_application::{UserInfo, ports::tenants::TenantRuntimeSnapshot};
use ryframe_kernel::{ActorContext, AppError, AppResult, DataScope};

use crate::{
    dto::{
        auth_dto::{SessionContextVo, SessionUserVo, TenantBusinessDataContextVo},
        product_dto::SessionProductContextVo,
        public_dto::MenuTreeNode,
    },
    state::AppState,
};

const SNAPSHOT_RETRIES: usize = 2;

#[derive(Clone, Debug)]
pub(crate) struct TenantContextHeaderValues {
    pub authorization_epoch: String,
    pub runtime_epoch: String,
    pub data_generation: String,
    pub data_state: String,
}

#[utoipa::path(
    get,
    path = "/api/v1/auth/context",
    tag = "认证",
    responses(
        (status = 200, description = "当前原子会话上下文", body = ApiResponse<SessionContextVo>),
        (status = 401, description = "未认证或会话失效"),
        (status = 503, description = "控制库上下文暂不可用")
    ),
    security(("bearer" = []))
)]
pub async fn context(
    State(state): State<AppState>,
    principal: RequestPrincipal,
) -> HttpResult<Response> {
    let context = build_session_context(&state, &principal.actor).await?;
    let headers = TenantContextHeaderValues {
        authorization_epoch: context.authorization_epoch.to_string(),
        runtime_epoch: context.runtime_epoch.clone(),
        data_generation: context.business_data.placement_generation.clone(),
        data_state: context.business_data.state.as_str().to_owned(),
    };
    let mut response = Json(ApiResponse::success(context)).into_response();
    response.extensions_mut().insert(headers);
    Ok(response)
}

/// 登录和刷新也必须调用这一构建函数，避免三条入口的授权、菜单或能力投影漂移。
pub(super) async fn build_session_context(
    state: &AppState,
    actor: &ActorContext,
) -> AppResult<SessionContextVo> {
    for _ in 0..SNAPSHOT_RETRIES {
        let before = state
            .services
            .tenant_data
            .runtime_snapshot(&actor.tenant_id)
            .await?;
        let user = state.services.auth.get_current_user(actor).await?;
        let service_product = state
            .services
            .product
            .session_context(&actor.tenant_id)
            .await?;
        let excluded_routes = state
            .services
            .product
            .disabled_session_route_keys(&service_product);
        let product = SessionProductContextVo::try_from(service_product)?;
        let menus = state
            .services
            .menu
            .find_session_tree(actor, &user.perms, &excluded_routes)
            .await?
            .into_iter()
            .map(MenuTreeNode::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let after = state
            .services
            .tenant_data
            .runtime_snapshot(&actor.tenant_id)
            .await?;

        if before == after
            && product.runtime_epoch == before.runtime_epoch().to_string()
            && product.authorization_epoch.parse::<u64>().ok() == Some(before.authorization_epoch())
        {
            return assemble_context(user, product, menus, &before);
        }
    }
    Err(AppError::ServiceUnavailable(
        "租户上下文正在更新，请重新发起请求".into(),
    ))
}

fn assemble_context(
    mut user: UserInfo,
    product: SessionProductContextVo,
    menus: Vec<MenuTreeNode>,
    snapshot: &TenantRuntimeSnapshot,
) -> AppResult<SessionContextVo> {
    let is_super_admin = user.is_super_admin;
    let roles = std::mem::take(&mut user.roles);
    let permissions = std::mem::take(&mut user.perms);
    Ok(SessionContextVo {
        user: SessionUserVo::from(user),
        is_super_admin,
        roles,
        permissions,
        authorization_epoch: snapshot.authorization_epoch().to_string(),
        runtime_epoch: snapshot.runtime_epoch().to_string(),
        capabilities: product.capabilities,
        business_data: TenantBusinessDataContextVo {
            state: snapshot.business_data_state().into(),
            placement_generation: snapshot.placement_generation().to_string(),
        },
        menus,
    })
}

pub fn login_actor(user_id: i64, user: &UserInfo) -> ActorContext {
    ActorContext {
        user_id,
        tenant_id: user.tenant_id.clone(),
        username: user.username.clone(),
        dept_id: None,
        dept_path: None,
        data_scope: DataScope::SelfOnly,
        custom_dept_ids: Vec::new(),
        include_self: true,
        is_super_admin: user.is_super_admin,
    }
}
