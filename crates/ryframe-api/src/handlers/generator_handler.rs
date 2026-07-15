use axum::{Json, Router, body::Body, extract::State, http::header, response::IntoResponse};
use ryframe_common::{ApiResponse, AppResult};
use ryframe_generator::{GenerateOptions, GeneratedFile};
use ryframe_macro::{get, post, route};

use crate::handlers::auth_handler::AppState;

pub fn generator_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_tables))
        .merge(route!(preview))
        .merge(route!(generate))
        .merge(route!(download))
        .with_state(state)
}

/// 列出数据库表
#[get("/tables")]
#[perm("tools:gen:list")]
async fn list_tables(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<Vec<ryframe_generator::TableInfo>>>> {
    let tables = state.generator_service.list_tables(&state.db).await?;
    Ok(Json(ApiResponse::success(tables)))
}

/// 预览生成内容
#[post("/preview")]
#[perm("tools:gen:list")]
async fn preview(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptions>,
) -> AppResult<Json<ApiResponse<Vec<GeneratedFile>>>> {
    let files = state.generator_service.preview(&state.db, opts).await?;
    Ok(Json(ApiResponse::success(files)))
}

/// 写入磁盘
#[post("/generate")]
#[perm("tools:gen:add")]
async fn generate(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptions>,
) -> AppResult<Json<ApiResponse<Vec<String>>>> {
    let written = state.generator_service.generate(&state.db, opts).await?;
    Ok(Json(ApiResponse::success(written)))
}

/// 打包 zip 下载
#[post("/download")]
#[perm("tools:gen:add")]
async fn download(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptions>,
) -> Result<impl IntoResponse, ryframe_common::AppError> {
    let zip_data = state
        .generator_service
        .download_zip(&state.db, opts)
        .await?;

    let headers = [
        (header::CONTENT_TYPE, "application/zip"),
        (
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"ryframe-gen.zip\"",
        ),
    ];

    Ok((headers, Body::from(zip_data)))
}
