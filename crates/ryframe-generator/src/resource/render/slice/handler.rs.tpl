{header}use std::sync::Arc;

use axum::{{
    Json, Router,
    extract::{{Path, Query, State}},
}};
use ryframe_application::generated::{name}::{pascal}Service;
use ryframe_kernel::{{AppError, PaginationPolicy}};
use ryframe_macro::{{delete, get, post, put, route}};
use validator::Validate;

use crate::RequestPrincipal;
use crate::handler_utils::parse_id;
use crate::http::{{ApiPageResponse, ApiResponse, HttpAppError, HttpResult}};

use super::dto::{{Create{pascal}Dto, {pascal}ListQuery, {pascal}Vo{detail_import}, Update{pascal}Dto}};

#[derive(Clone)]
pub struct {pascal}HttpState {{
    service: Arc<{pascal}Service>,
    pagination: PaginationPolicy,
}}

pub fn router(service: Arc<{pascal}Service>, pagination: PaginationPolicy) -> Router {{
    Router::new()
        .merge(route!(list))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .with_state({pascal}HttpState {{ service, pagination }})
}}

#[get("/")]
{capability_attribute}#[perm({list_permission:?})]
#[utoipa::path(
    get,
    path = {path:?},
    operation_id = {list_operation:?},
    tag = {tag:?},
    params({pascal}ListQuery),
    responses((status = 200, description = "列表", body = ApiPageResponse<{pascal}Vo>)),
    security(("bearer" = []))
)]
pub async fn list(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Query(query): Query<{pascal}ListQuery>,
) -> HttpResult<Json<ApiPageResponse<{pascal}Vo>>> {{
    let page = state
        .service
        .find_by_page(&current_user, query.into_service_params(state.pagination)?)
        .await
        .map_err(HttpAppError::from)?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map({pascal}Vo::from).collect(),
        page.total,
        page.page,
        page.page_size,
        state.pagination.max_page_size(),
    )))
}}

#[get("/{{id}}")]
{capability_attribute}#[perm({read_permission:?})]
#[utoipa::path(
    get,
    path = {detail_path:?},
    operation_id = {read_operation:?},
    tag = {tag:?},
    params(("id" = String, Path)),
    responses((status = 200, description = "详情", body = ApiResponse<{detail_vo}>)),
    security(("bearer" = []))
)]
pub async fn detail(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<{detail_vo}>>> {{
    let value = state
        .service
        .find_by_id(&current_user, parse_id(&id, "id 必须是 i64 字符串")?)
        .await
        .map_err(HttpAppError::from)?
        .ok_or_else(|| HttpAppError::from(AppError::NotFound({not_found:?}.into())))?;
    Ok(Json(ApiResponse::success(value.into())))
}}

#[post("/")]
{capability_attribute}#[perm({create_permission:?})]
#[utoipa::path(
    post,
    path = {path:?},
    operation_id = {create_operation:?},
    tag = {tag:?},
    request_body = Create{pascal}Dto,
    responses((status = 200, description = "创建成功", body = ApiResponse<{pascal}Vo>)),
    security(("bearer" = []))
)]
pub async fn create(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Json(dto): Json<Create{pascal}Dto>,
) -> HttpResult<Json<ApiResponse<{pascal}Vo>>> {{
    dto.validate()?;
    let value = state
        .service
        .create(&current_user, dto.into())
        .await
        .map_err(HttpAppError::from)?;
    Ok(Json(ApiResponse::success(value.into())))
}}

#[put("/{{id}}")]
{capability_attribute}#[perm({update_permission:?})]
#[utoipa::path(
    put,
    path = {detail_path:?},
    operation_id = {update_operation:?},
    tag = {tag:?},
    params(("id" = String, Path)),
    request_body = Update{pascal}Dto,
    responses((status = 200, description = "更新成功", body = ApiResponse<{pascal}Vo>)),
    security(("bearer" = []))
)]
pub async fn update(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Json(dto): Json<Update{pascal}Dto>,
) -> HttpResult<Json<ApiResponse<{pascal}Vo>>> {{
    dto.validate()?;
    let value = state
        .service
        .update(
            &current_user,
            parse_id(&id, "id 必须是 i64 字符串")?,
            dto.into(),
        )
        .await
        .map_err(HttpAppError::from)?;
    Ok(Json(ApiResponse::success(value.into())))
}}

#[delete("/{{id}}")]
{capability_attribute}#[perm({delete_permission:?})]
#[utoipa::path(
    delete,
    path = {detail_path:?},
    operation_id = {delete_operation:?},
    tag = {tag:?},
    params(("id" = String, Path)),
    responses((status = 200, description = "删除成功", body = crate::http::ApiEmptyResponse)),
    security(("bearer" = []))
)]
pub async fn remove(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<()>>> {{
    state
        .service
        .delete(&current_user, parse_id(&id, "id 必须是 i64 字符串")?)
        .await
        .map_err(HttpAppError::from)?;
    Ok(Json(ApiResponse::success_no_data()))
}}
