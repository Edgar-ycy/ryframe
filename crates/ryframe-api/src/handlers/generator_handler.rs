use axum::{
    Json, Router,
    body::Body,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::IntoResponse,
};
use ryframe_adapters::ValidatedPageQuery;
use ryframe_application::system::generator_service::TableListParams;
use ryframe_config::{Environment, PaginationConfig};
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_kernel::{AppError, AppResult};
use ryframe_macro::{get, post, route};
use serde::Deserialize;

use crate::{
    dto::{
        generator_dto::{GenerateOptionsDto, GenerateRequestDto},
        public_dto::{GeneratedFile, TableInfo, WriteReport},
    },
    handler_utils::attachment_content_disposition,
    state::AppState,
};

pub fn generator_router(state: AppState) -> Router {
    let router = Router::new()
        .merge(route!(list_tables))
        .merge(route!(preview))
        .merge(route!(download));
    let router = if online_write_enabled(state.config.environment) {
        router.merge(route!(generate))
    } else {
        router
    };
    router.with_state(state)
}

const fn online_write_enabled(environment: Environment) -> bool {
    !environment.is_production()
}

fn ensure_online_write_enabled(environment: Environment) -> AppResult<()> {
    if online_write_enabled(environment) {
        Ok(())
    } else {
        Err(AppError::CapabilityUnavailable(
            "生产环境禁用在线代码生成写盘，请使用独立命令行工具".into(),
        ))
    }
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
    fn into_service_params(self, policy: &PaginationConfig) -> HttpResult<TableListParams> {
        Ok(TableListParams {
            page: ValidatedPageQuery::from_optional(self.page, self.page_size, policy)?,
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
) -> HttpResult<Json<ApiPageResponse<TableInfo>>> {
    state
        .services
        .generator
        .list_tables(query.into_service_params(&state.config.pagination)?)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|page| {
            Json(ApiPageResponse::page(
                page.records.into_iter().map(TableInfo::from).collect(),
                page.total,
                page.page,
                page.page_size,
                state.config.pagination.max_page_size,
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
) -> HttpResult<Json<ApiResponse<Vec<GeneratedFile>>>> {
    let files = state.services.generator.preview(opts.into()).await?;
    Ok(Json(ApiResponse::success(
        files.into_iter().map(GeneratedFile::from).collect(),
    )))
}

/// 写入磁盘
#[post("/generate")]
#[perm("tools:gen:add")]
#[utoipa::path(post, path = "/api/v1/tools/gen/generate", tag = "代码生成",
    responses((status = 200, description = "代码生成报告", body = ApiResponse<WriteReport>)), security(("bearer" = [])))]
async fn generate(
    State(state): State<AppState>,
    Json(request): Json<GenerateRequestDto>,
) -> HttpResult<Json<ApiResponse<WriteReport>>> {
    ensure_online_write_enabled(state.config.environment)?;
    let written = state
        .services
        .generator
        .generate(request.options.into(), request.output_dir.into())
        .await?;
    Ok(Json(ApiResponse::success(written.into())))
}

/// 打包 zip 下载
#[post("/download")]
#[perm("tools:gen:add")]
#[utoipa::path(post, path = "/api/v1/tools/gen/download", tag = "代码生成",
    responses((status = 200, description = "下载生成代码", body = Vec<u8>, content_type = "application/zip")), security(("bearer" = [])))]
async fn download(
    State(state): State<AppState>,
    Json(opts): Json<GenerateOptionsDto>,
) -> HttpResult<impl IntoResponse> {
    let zip_data = state.services.generator.download_zip(opts.into()).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        attachment_content_disposition("ryframe-gen.zip")?,
    );

    Ok((headers, Body::from(zip_data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_disables_online_disk_writes() {
        assert!(!online_write_enabled(Environment::Prod));
        assert!(matches!(
            ensure_online_write_enabled(Environment::Prod),
            Err(AppError::CapabilityUnavailable(_))
        ));
    }

    #[test]
    fn isolated_environments_keep_online_disk_writes_available() {
        for environment in [Environment::Dev, Environment::Test] {
            assert!(online_write_enabled(environment));
            assert!(ensure_online_write_enabled(environment).is_ok());
        }
    }
}
