use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiResponse, AppResult};
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{
    CreatePermissionCommand, PermissionSyncReport, PermissionTreeNode, PermissionType,
    PermissionVo, UpdatePermissionCommand,
};
use validator::Validate;

use crate::dto::permission_dto::{CreatePermissionDto, UpdatePermissionDto};
use crate::handler_utils::parse_optional_i64;
use crate::state::AppState;

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct PermissionListQuery {
    pub perm_type: Option<PermissionType>,
}

pub fn permission_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(tree))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .merge(route!(sync_perm_from_route))
        .with_state(state)
}

#[get("/tree")]
#[perm("system:perm:list")]
#[utoipa::path(get, path = "/api/v1/system/perms/tree", tag = "权限管理",
    params(PermissionListQuery),
    responses((status = 200, description = "权限树", body = ApiResponse<Vec<PermissionTreeNode>>)), security(("bearer" = [])))]
pub async fn tree(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<PermissionListQuery>,
) -> AppResult<Json<ApiResponse<Vec<PermissionTreeNode>>>> {
    let perm_type = query.perm_type.map(PermissionType::as_str);
    let tree = state
        .services
        .permission
        .list_all_perms(&current_user, perm_type)
        .await?;
    Ok(Json(ApiResponse::success(tree)))
}

#[get("/{id}")]
#[perm("system:perm:list")]
#[utoipa::path(get, path = "/api/v1/system/perms/{id}", tag = "权限管理",
    params(("id" = i64, Path)), responses((status = 200, description = "权限详情", body = ApiResponse<PermissionVo>)),
    security(("bearer" = [])))]
pub async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<PermissionVo>>> {
    let item = state
        .services
        .permission
        .find_by_id(&current_user, id)
        .await?;
    match item {
        Some(item) => Ok(Json(ApiResponse::success(item))),
        None => Err(ryframe_common::AppError::NotFound("权限不存在".into())),
    }
}

#[post("/")]
#[perm("system:perm:add")]
#[utoipa::path(post, path = "/api/v1/system/perms", tag = "权限管理",
    request_body = CreatePermissionDto, responses((status = 200, description = "创建成功", body = ApiResponse<PermissionVo>)),
    security(("bearer" = [])))]
pub async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreatePermissionDto>,
) -> AppResult<Json<ApiResponse<PermissionVo>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id)?;
    let item = state
        .services
        .permission
        .create(
            &current_user,
            CreatePermissionCommand {
                name: dto.name,
                code: dto.code,
                parent_id,
                perm_type: dto.perm_type,
                icon: dto.icon,
                sort: dto.sort.unwrap_or(0),
                status: dto.status.unwrap_or_else(|| "1".into()),
            },
        )
        .await?;
    Ok(Json(ApiResponse::success(item)))
}

#[put("/{id}")]
#[perm("system:perm:edit")]
#[utoipa::path(put, path = "/api/v1/system/perms/{id}", tag = "权限管理",
    params(("id" = i64, Path)), request_body = UpdatePermissionDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<PermissionVo>)), security(("bearer" = [])))]
pub async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdatePermissionDto>,
) -> AppResult<Json<ApiResponse<PermissionVo>>> {
    dto.validate()?;
    let parent_id = parse_optional_i64(dto.parent_id)?;
    let item = state
        .services
        .permission
        .update(
            &current_user,
            UpdatePermissionCommand {
                id,
                name: dto.name,
                code: dto.code,
                parent_id,
                perm_type: dto.perm_type,
                icon: dto.icon,
                sort: dto.sort.unwrap_or(0),
                status: dto.status.unwrap_or_else(|| "1".into()),
            },
        )
        .await?;
    state
        .services
        .menu
        .invalidate_menu_cache(&current_user.tenant_id)
        .await;
    Ok(Json(ApiResponse::success(item)))
}

#[delete("/{id}")]
#[perm("system:perm:remove")]
#[utoipa::path(delete, path = "/api/v1/system/perms/{id}", tag = "权限管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_common::ApiEmptyResponse)),
    security(("bearer" = [])))]
pub async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.services.permission.delete(&current_user, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

#[post("/sync")]
#[perm("system:perm:sync")]
#[utoipa::path(post, path = "/api/v1/system/perms/sync", tag = "权限管理",
    responses((status = 200, description = "同步成功", body = ApiResponse<PermissionSyncReport>)), security(("bearer" = [])))]
pub async fn sync_perm_from_route(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<PermissionSyncReport>>> {
    let report = state
        .services
        .permission
        .sync_route_permissions(
            &current_user,
            crate::permission_catalog::route_permission_codes(),
        )
        .await?;
    Ok(Json(ApiResponse::success(report)))
}
