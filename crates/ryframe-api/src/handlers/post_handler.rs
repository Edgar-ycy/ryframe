use crate::RequestPrincipal;
use crate::http::{ApiPageResponse, ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_application::system::PostListParams;
use ryframe_kernel::ValidatedPageQuery;
use ryframe_macro::{delete, get, post, put, route};
use validator::Validate;

use crate::dto::post_dto::{CreatePostDto, UpdatePostDto};
use crate::dto::public_dto::{ExportJobVo, PostVo};
use crate::state::AppState;
use crate::{detail_body, list_query, remove_body};
use crate::{dto::export_dto::PostExportRequestDto, handlers::export_handler::request_export};

list_query!(pub PostListQuery, PostFilterQuery {
    name: String,
    code: String,
    status: String,
});

impl PostFilterQuery {
    fn into_service_params(self, page: ValidatedPageQuery) -> PostListParams {
        PostListParams {
            page,
            name: self.name,
            code: self.code,
            status: self.status,
        }
    }
}

pub fn post_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(request_post_export))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .with_state(state)
}

/// 岗位列表分页查询
#[get("/")]
#[perm("system:post:list")]
#[utoipa::path(get, path = "/api/v1/system/posts", tag = "岗位管理",
    params(PostListQuery),
    responses((status = 200, description = "岗位列表", body = ApiPageResponse<PostVo>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<PostListQuery>,
) -> HttpResult<Json<ApiPageResponse<PostVo>>> {
    let (page, filter) = query.into_parts(state.pagination)?;
    state
        .services
        .post
        .find_by_page(&current_user, filter.into_service_params(page))
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(|p| {
            Json(ApiPageResponse::page(
                p.records.into_iter().map(PostVo::from).collect(),
                p.total,
                p.page,
                p.page_size,
                state.pagination.max_page_size(),
            ))
        })
}

/// 岗位详情
#[get("/{id}")]
#[perm("system:post:list")]
#[utoipa::path(get, path = "/api/v1/system/posts/{id}", tag = "岗位管理",
    params(("id" = String, Path)),
    responses((status = 200, description = "岗位详情", body = ApiResponse<PostVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<PostVo>>> {
    detail_body!(state, current_user, id, post, PostVo, "岗位")
}

/// 创建岗位
#[post("/")]
#[perm("system:post:add")]
#[utoipa::path(post, path = "/api/v1/system/posts", tag = "岗位管理",
    request_body = CreatePostDto, responses((status = 200, description = "创建成功", body = ApiResponse<PostVo>)), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreatePostDto>,
) -> HttpResult<Json<ApiResponse<PostVo>>> {
    dto.validate()?;
    state
        .services
        .post
        .create(&current_user, &dto.name, &dto.code, dto.sort.unwrap_or(0))
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 更新岗位
#[put("/{id}")]
#[perm("system:post:edit")]
#[utoipa::path(put, path = "/api/v1/system/posts/{id}", tag = "岗位管理",
    params(("id" = String, Path)), request_body = UpdatePostDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<PostVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdatePostDto>,
) -> HttpResult<Json<ApiResponse<PostVo>>> {
    dto.validate()?;
    state
        .services
        .post
        .update(
            &current_user,
            id,
            &dto.name,
            dto.sort.unwrap_or(0),
            dto.status,
        )
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 删除岗位
#[delete("/{id}")]
#[perm("system:post:remove")]
#[utoipa::path(delete, path = "/api/v1/system/posts/{id}", tag = "岗位管理",
    params(("id" = String, Path)), responses((status = 200, description = "删除成功", body = crate::http::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<()>>> {
    remove_body!(state, current_user, id, post)
}

/// 创建岗位异步导出任务。
#[post("/exports")]
#[perm("system:post:export")]
#[utoipa::path(post, path = "/api/v1/system/posts/exports", tag = "岗位管理",
    params(("Idempotency-Key" = String, Header, description = "幂等键")), request_body = PostExportRequestDto,
    responses((status = 202, description = "岗位导出任务已创建", body = ApiResponse<ExportJobVo>)), security(("bearer" = [])))]
async fn request_post_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<PostExportRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    let (selection, confirm_all) = request.into_selection();
    request_export(
        state,
        current_user,
        headers,
        "system:post:export",
        selection,
        confirm_all,
    )
    .await
}
