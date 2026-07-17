use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::header,
    response::IntoResponse,
};
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{get, post, route};
use ryframe_service::system::generator_service::{
    GeneratedFile, TableInfo, TableListParams, WriteReport,
};
use serde::Deserialize;

use crate::{
    dto::generator_dto::{GenerateOptionsDto, GenerateRequestDto},
    state::AppState,
};

pub fn generator_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list_tables))
        .merge(route!(preview))
        .merge(route!(generate))
        .merge(route!(download))
        .with_state(state)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
struct TableListQuery {
    #[serde(default = "ryframe_core::repository::default_page")]
    page: u64,
    #[serde(default = "ryframe_core::repository::default_page_size")]
    page_size: u64,
    table_name: Option<String>,
    table_comment: Option<String>,
}

impl TableListQuery {
    fn into_service_params(self) -> TableListParams {
        TableListParams {
            page: PageQuery {
                page: self.page,
                page_size: self.page_size,
            },
            table_name: self.table_name,
            table_comment: self.table_comment,
        }
    }
}

/// 列出数据库表
#[get("/tables")]
#[perm("tools:gen:list")]
#[utoipa::path(get, path = "/api/v1/tools/gen/tables", tag = "代码生成",
    params(TableListQuery),
    responses((status = 200, description = "数据库表列表", body = ApiPageResponse<TableInfo>)), security(("bearer" = [])))]
async fn list_tables(
    State(state): State<AppState>,
    Query(query): Query<TableListQuery>,
) -> AppResult<Json<ApiPageResponse<TableInfo>>> {
    state
        .services
        .generator
        .list_tables(query.into_service_params())
        .await
        .map(|page| Json(page.to_page_response("查询成功")))
}

/// 预览生成内容
#[post("/preview")]
#[perm("tools:gen:list")]
#[utoipa::path(post, path = "/api/v1/tools/gen/preview", tag = "代码生成",
    responses((status = 200, description = "生成结果预览", body = ApiResponse<Vec<GeneratedFile>>)), security(("bearer" = [])))]
async fn preview(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptionsDto>,
) -> AppResult<Json<ApiResponse<Vec<GeneratedFile>>>> {
    let files = state.services.generator.preview(opts.into()).await?;
    Ok(Json(ApiResponse::success(files)))
}

/// 写入磁盘
#[post("/generate")]
#[perm("tools:gen:add")]
#[utoipa::path(post, path = "/api/v1/tools/gen/generate", tag = "代码生成",
    responses((status = 200, description = "代码生成报告", body = ApiResponse<WriteReport>)), security(("bearer" = [])))]
async fn generate(
    State(state): State<AppState>,
    Json(request): Json<GenerateRequestDto>,
) -> AppResult<Json<ApiResponse<WriteReport>>> {
    let written = state
        .services
        .generator
        .generate(request.options.into(), request.output_dir.into())
        .await?;
    Ok(Json(ApiResponse::success(written)))
}

/// 打包 zip 下载
#[post("/download")]
#[perm("tools:gen:add")]
#[utoipa::path(post, path = "/api/v1/tools/gen/download", tag = "代码生成",
    responses((status = 200, description = "下载生成代码", body = Vec<u8>, content_type = "application/zip")), security(("bearer" = [])))]
async fn download(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptionsDto>,
) -> Result<impl IntoResponse, ryframe_common::AppError> {
    let zip_data = state.services.generator.download_zip(opts.into()).await?;

    let headers = [
        (header::CONTENT_TYPE, "application/zip"),
        (
            header::CONTENT_DISPOSITION,
            "attachment; filename=\"ryframe-gen.zip\"",
        ),
    ];

    Ok((headers, Body::from(zip_data)))
}
