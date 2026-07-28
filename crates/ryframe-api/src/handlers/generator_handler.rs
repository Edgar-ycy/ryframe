use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::header,
    response::IntoResponse,
};
use ryframe_config::PaginationConfig;
use ryframe_core::PageQuery;
use ryframe_http::{ApiPageResponse, ApiResponse, AppResult};
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
    /// 页码，从 1 开始。
    #[param(minimum = 1)]
    page: Option<u64>,
    /// 每页记录数，受 `pagination.max_page_size` 限制（默认值为 100）。
    #[param(minimum = 1)]
    page_size: Option<u64>,
    table_name: Option<String>,
    table_comment: Option<String>,
}

impl TableListQuery {
    fn into_service_params(self, policy: &PaginationConfig) -> AppResult<TableListParams> {
        Ok(TableListParams {
            page: PageQuery::from_optional(self.page, self.page_size, policy)?,
            table_name: self.table_name,
            table_comment: self.table_comment,
        })
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
        .list_tables(query.into_service_params(&state.config.pagination)?)
        .await
        .map_err(ryframe_http::AppError::from)
        .map(|page| {
            Json(ApiPageResponse::new(
                page.records,
                page.total,
                page.page,
                page.page_size,
                state.config.pagination.max_page_size,
                "查询成功",
            ))
        })
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
) -> Result<impl IntoResponse, ryframe_http::AppError> {
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
