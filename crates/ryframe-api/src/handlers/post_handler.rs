use serde::{Deserialize, Serialize};
use serde_json;
use axum::{extract::{Path, Query, State}, routing::get, Json, Router};
use ryframe_common::AppResult;
use ryframe_core::PageQuery;
use ryframe_service::system::PostVo;
use validator::Validate;
use crate::dto::post_dto::{CreatePostDto, UpdatePostDto};

use super::auth_handler::AppState;

/// 岗位列表查询参数（支持搜索过滤）
#[derive(Debug, Deserialize)]
pub struct PostListQuery {
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size")]
    pub page_size: u64,
    pub name: Option<String>,
    pub code: Option<String>,
    pub status: Option<String>,
}

fn default_page_size() -> u64 {
    10
}

pub fn post_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/export", get(export_posts))
        .route("/{id}", get(detail).put(update).delete(remove))
        .with_state(state)
}

/// 岗位列表分页查询
#[utoipa::path(get, path = "/api/v1/system/posts", tag = "岗位管理",
    responses((status = 200, description = "岗位列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    Query(query): Query<PostListQuery>,
) -> AppResult<Json<ryframe_core::PageResult<PostVo>>> {
    let page_query = PageQuery { page: query.page, page_size: query.page_size };
    let has_filter = query.name.is_some() || query.code.is_some() || query.status.is_some();
    if has_filter {
        state.post_service
            .find_by_page_filtered(&state.db, page_query, query.name.as_deref(), query.code.as_deref(), query.status.as_deref())
            .await.map(Json)
    } else {
        state.post_service.find_by_page(&state.db, page_query).await.map(Json)
    }
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<PostVo>> {
    match state.post_service.find_by_id(&state.db, id).await? {
        Some(post) => Ok(Json(post)),
        None => Err(ryframe_common::AppError::NotFound("岗位不存在".into())),
    }
}

/// 创建岗位
#[utoipa::path(post, path = "/api/v1/system/posts", tag = "岗位管理",
    request_body = CreatePostDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(State(state): State<AppState>, Json(dto): Json<CreatePostDto>) -> AppResult<Json<PostVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.post_service.create(&state.db, &dto.name, &dto.code, dto.sort.unwrap_or(0)).await.map(Json)
}

/// 更新岗位
#[utoipa::path(put, path = "/api/v1/system/posts/{id}", tag = "岗位管理",
    params(("id" = i64, Path)), request_body = UpdatePostDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(State(state): State<AppState>, Path(id): Path<i64>, Json(dto): Json<UpdatePostDto>) -> AppResult<Json<PostVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.post_service.update(&state.db, id, &dto.name, dto.sort.unwrap_or(0), dto.status).await.map(Json)
}

/// 删除岗位
#[utoipa::path(delete, path = "/api/v1/system/posts/{id}", tag = "岗位管理",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<serde_json::Value>> {
    state.post_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}

/// 岗位导出数据
#[derive(Debug, Serialize)]
struct PostExportData {
    pub post_id: i64,
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
async fn export_posts(
    State(state): State<AppState>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let all_posts = state.post_service.find_all(&state.db).await?;
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

    let bytes = ExcelExporter::export_to_bytes(
        &export_data,
        "岗位数据",
        &PostExportData::excel_headers(),
    )?;

    let response = axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        .header("Content-Disposition", "attachment; filename=posts.xlsx")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ryframe_common::AppError::Internal(format!("构建响应失败: {}", e)))?;

    Ok(response)
}
