use crate::{naming, schema::TableInfo};

pub fn render_handler(table: &TableInfo, _module: &str) -> String {
    let struct_name = naming::to_pascal_case(&table.table_name);
    let snake = naming::to_snake_case(&table.table_name);
    let pk_type = crate::schema::get_pk_type(table);

    format!(
        r#"use axum::{{extract::{{Path, Query, State}}, routing::{{get, post, put, delete}}, Json, Router}};
use ryframe_common::{{ApiPageResponse, ApiResponse, AppResult}};
use serde::Deserialize;
use std::sync::Arc;

use crate::dto::{snake}_dto::*;
use super::super::service::{snake}_service::{snake_name}ServiceImpl;

/// {snake_name} 路由状态
#[derive(Clone)]
pub struct {snake_name}State {{
    pub service: Arc<{snake_name}ServiceImpl>,
}}

/// 创建 {snake} 路由（需传入数据库连接）
pub fn {snake}_router(db: sea_orm::DatabaseConnection) -> Router {{
    let service = Arc::new({snake_name}ServiceImpl::new(db));
    let state = {snake_name}State {{ service }};
    Router::new()
        .route("/", get(list).post(create))
        .route("/{{id}}", get(detail).put(update).delete(remove))
        .with_state(state)
}}

/// 列表查询参数
#[derive(Debug, Deserialize)]
pub struct {snake_name}ListQuery {{
    #[serde(default)]
    pub page: u64,
    #[serde(default = "default_page_size", alias = "pageSize")]
    pub page_size: u64,
}}

fn default_page_size() -> u64 {{ 10 }}

/// 分页列表
async fn list(
    State(state): State<{snake_name}State>,
    Query(query): Query<{snake_name}ListQuery>,
) -> AppResult<Json<ApiPageResponse<{snake_name}Vo>>> {{
    let page_query = ryframe_core::PageQuery {{
        page: query.page,
        page_size: query.page_size,
    }};
    state.service.list(&page_query).await
        .map(|p| Json(p.to_page_response("查询成功")))
}}

/// 详情
async fn detail(
    State(state): State<{snake_name}State>,
    Path(id): Path<{pk_type}>,
) -> AppResult<Json<ApiResponse<{snake_name}Vo>>> {{
    match state.service.find_by_id(id).await? {{
        Some(v) => Ok(Json(ApiResponse::success(v))),
        None => Err(ryframe_common::AppError::NotFound("记录不存在".into())),
    }}
}}

/// 创建
async fn create(
    State(state): State<{snake_name}State>,
    Json(dto): Json<Create{snake_name}Dto>,
) -> AppResult<Json<ApiResponse<{snake_name}Vo>>> {{
    state.service.create(dto).await
        .map(|v| Json(ApiResponse::success(v)))
}}

/// 更新
async fn update(
    State(state): State<{snake_name}State>,
    Path(id): Path<{pk_type}>,
    Json(dto): Json<Update{snake_name}Dto>,
) -> AppResult<Json<ApiResponse<{snake_name}Vo>>> {{
    state.service.update(id, dto).await
        .map(|v| Json(ApiResponse::success(v)))
}}

/// 删除
async fn remove(
    State(state): State<{snake_name}State>,
    Path(id): Path<{pk_type}>,
) -> AppResult<Json<ApiResponse<serde_json::Value>>> {{
    state.service.delete(id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}}
"#,
        snake_name = struct_name,
        snake = snake,
        pk_type = pk_type,
    )
}
