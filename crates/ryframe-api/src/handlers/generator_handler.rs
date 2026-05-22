use axum::{
    body::Body,
    extract::State,
    http::header,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use ryframe_common::AppResult;
use ryframe_generator::{GenerateOptions, GeneratedFile};
use serde::Deserialize;

use crate::handlers::auth_handler::AppState;

/// 表查询参数
#[derive(Debug, Deserialize)]
pub struct GenTableQuery {
    pub table_name: Option<String>,
}

pub fn generator_router(state: AppState) -> Router {
    Router::new()
        .route("/tables", get(list_tables))
        .route("/preview", post(preview))
        .route("/generate", post(generate))
        .route("/download", post(download))
        .with_state(state)
}

/// 列出数据库表
async fn list_tables(
    State(state): State<AppState>,
) -> AppResult<Json<Vec<ryframe_generator::TableInfo>>> {
    let tables = state.generator_service.list_tables(&state.db).await?;
    Ok(Json(tables))
}

/// 预览生成内容
async fn preview(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptions>,
) -> AppResult<Json<Vec<GeneratedFile>>> {
    let files = state.generator_service.preview(&state.db, opts).await?;
    Ok(Json(files))
}

/// 写入磁盘
async fn generate(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptions>,
) -> AppResult<Json<Vec<String>>> {
    let written = state.generator_service.generate(&state.db, opts).await?;
    Ok(Json(written))
}

/// 打包 zip 下载
async fn download(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptions>,
) -> Result<impl IntoResponse, ryframe_common::AppError> {
    let zip_data = state.generator_service.download_zip(&state.db, opts).await?;

    let headers = [
        (header::CONTENT_TYPE, "application/zip"),
        (
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"ryframe-gen.zip\"",
        ),
    ];

    Ok((headers, Body::from(zip_data)))
}
