use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::{PostListParams, PostVo};
use serde::Serialize;
use validator::Validate;

use crate::dto::post_dto::{CreatePostDto, UpdatePostDto};
use crate::handler_utils::excel_response;
use crate::state::AppState;
use crate::{detail_body, list_query, remove_body};

list_query!(pub PostListQuery, PostFilterQuery {
    name: String,
    code: String,
    status: String,
});

impl PostFilterQuery {
    fn into_service_params(self, page: PageQuery) -> PostListParams {
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
        .merge(route!(list_no_page))
        .merge(route!(export_posts))
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
) -> AppResult<Json<ApiPageResponse<PostVo>>> {
    let (page, filter) = query.into_parts();
    state
        .services
        .post
        .find_by_page(&current_user, filter.into_service_params(page))
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 岗位列表不分页查询（返回全部数据）
#[get("/all")]
#[perm("system:post:list")]
#[utoipa::path(get, path = "/api/v1/system/posts/all", tag = "岗位管理",
    params(PostFilterQuery),
    responses((status = 200, description = "岗位列表", body = ApiResponse<Vec<PostVo>>)),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<PostFilterQuery>,
) -> AppResult<Json<ApiResponse<Vec<PostVo>>>> {
    state
        .services
        .post
        .find_by_page(
            &current_user,
            query.into_service_params(PageQuery::all_records()),
        )
        .await
        .map(|page| Json(ApiResponse::success(page.records)))
}

/// 岗位详情
#[get("/{id}")]
#[perm("system:post:list")]
#[utoipa::path(get, path = "/api/v1/system/posts/{id}", tag = "岗位管理",
    params(("id" = i64, Path)),
    responses((status = 200, description = "岗位详情", body = ApiResponse<PostVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<PostVo>>> {
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
) -> AppResult<Json<ApiResponse<PostVo>>> {
    dto.validate()?;
    state
        .services
        .post
        .create(&current_user, &dto.name, &dto.code, dto.sort.unwrap_or(0))
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新岗位
#[put("/{id}")]
#[perm("system:post:edit")]
#[utoipa::path(put, path = "/api/v1/system/posts/{id}", tag = "岗位管理",
    params(("id" = i64, Path)), request_body = UpdatePostDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<PostVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdatePostDto>,
) -> AppResult<Json<ApiResponse<PostVo>>> {
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
        .map(|v| Json(ApiResponse::success(v)))
}

/// 删除岗位
#[delete("/{id}")]
#[perm("system:post:remove")]
#[utoipa::path(delete, path = "/api/v1/system/posts/{id}", tag = "岗位管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    remove_body!(state, current_user, id, post)
}

/// 岗位导出数据
#[derive(Debug, Serialize)]
struct PostExportData {
    pub post_id: String,
    pub name: String,
    pub code: String,
    pub sort: i32,
    pub status: String,
    pub remark: Option<String>,
    pub created_at: String,
}

impl PostExportData {
    fn excel_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("post_id", "岗位ID"),
            ("name", "岗位名称"),
            ("code", "岗位编码"),
            ("sort", "排序"),
            ("status", "状态"),
            ("remark", "备注"),
            ("created_at", "创建时间"),
        ]
    }
}

/// 导出岗位数据为 Excel
#[get("/export")]
#[perm("system:post:export")]
#[utoipa::path(get, path = "/api/v1/system/posts/export", tag = "岗位管理",
    params(PostFilterQuery),
    responses((status = 200, description = "导出岗位 Excel", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")), security(("bearer" = [])))]
async fn export_posts(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<PostFilterQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let all_posts = state
        .services
        .post
        .find_by_page(
            &current_user,
            query.into_service_params(PageQuery::all_records()),
        )
        .await?
        .records;
    let export_data: Vec<PostExportData> = all_posts
        .into_iter()
        .map(|p| PostExportData {
            post_id: p.id,
            name: p.name,
            code: p.code,
            sort: p.sort,
            status: p.status,
            remark: p.remark,
            created_at: p.created_at.to_rfc3339(),
        })
        .collect();

    let bytes =
        ExcelExporter::export_to_bytes(&export_data, "岗位数据", &PostExportData::excel_headers())?;

    excel_response(bytes, "posts.xlsx")
}
