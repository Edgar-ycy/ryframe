use axum::{
    Json, Router,
    extract::{Path, Query, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiPageResponse, ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_macro::{delete, get, post, put, route};
use ryframe_service::system::ConfigVo;
use serde::Serialize;
use validator::Validate;

use crate::dto::config_dto::{
    ConfigFilterQuery, ConfigListQuery, CreateConfigDto, UpdateConfigDto,
};
use crate::handler_utils::excel_response;
use crate::state::AppState;

pub fn config_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(list_no_page))
        .merge(route!(export_configs))
        .merge(route!(refresh_cache))
        .merge(route!(get_by_key))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .with_state(state)
}

/// 参数配置列表
#[get("/")]
#[perm("system:config:list")]
#[utoipa::path(get, path = "/api/v1/system/configs", tag = "参数配置",
    params(ConfigListQuery),
    responses((status = 200, description = "配置列表", body = ApiPageResponse<ConfigVo>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<ConfigListQuery>,
) -> AppResult<Json<ryframe_common::ApiPageResponse<ConfigVo>>> {
    state
        .services
        .config
        .find_by_page(&current_user, query.into_service_params())
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 参数配置列表不分页查询（返回全部数据）
#[get("/all")]
#[perm("system:config:list")]
#[utoipa::path(get, path = "/api/v1/system/configs/all", tag = "参数配置",
    params(ConfigFilterQuery),
    responses((status = 200, description = "配置列表", body = ApiResponse<Vec<ConfigVo>>)),
    security(("bearer" = [])))]
async fn list_no_page(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<ConfigFilterQuery>,
) -> AppResult<Json<ApiResponse<Vec<ConfigVo>>>> {
    state
        .services
        .config
        .find_all(
            &current_user,
            query.into_service_params(PageQuery::all_records()),
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 参数配置详情
#[get("/{id}")]
#[perm("system:config:list")]
#[utoipa::path(get, path = "/api/v1/system/configs/{id}", tag = "参数配置",
    params(("id" = i64, Path)),
    responses((status = 200, description = "配置详情", body = ApiResponse<ConfigVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<ConfigVo>>> {
    match state.services.config.find_by_id(&current_user, id).await? {
        Some(cfg) => Ok(Json(ApiResponse::success(cfg))),
        None => Err(ryframe_common::AppError::NotFound("参数配置不存在".into())),
    }
}

/// 创建参数配置
#[post("/")]
#[perm("system:config:add")]
#[utoipa::path(post, path = "/api/v1/system/configs", tag = "参数配置",
    request_body = CreateConfigDto, responses((status = 200, description = "创建成功", body = ApiResponse<ConfigVo>)), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(dto): Json<CreateConfigDto>,
) -> AppResult<Json<ApiResponse<ConfigVo>>> {
    dto.validate()?;
    state
        .services
        .config
        .create(
            &current_user,
            &dto.name,
            &dto.key,
            &dto.value,
            dto.remark.as_deref(),
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新参数配置
#[put("/{id}")]
#[perm("system:config:edit")]
#[utoipa::path(put, path = "/api/v1/system/configs/{id}", tag = "参数配置",
    params(("id" = i64, Path)), request_body = UpdateConfigDto,
    responses((status = 200, description = "更新成功", body = ApiResponse<ConfigVo>)), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateConfigDto>,
) -> AppResult<Json<ApiResponse<ConfigVo>>> {
    dto.validate()?;
    state
        .services
        .config
        .update(&current_user, id, &dto.value)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 删除参数配置
#[delete("/{id}")]
#[perm("system:config:remove")]
#[utoipa::path(delete, path = "/api/v1/system/configs/{id}", tag = "参数配置",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.services.config.delete(&current_user, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 根据参数键名查询参数值
#[get("/key/{key}")]
#[perm("system:config:list")]
#[utoipa::path(get, path = "/api/v1/system/configs/key/{key}", tag = "参数配置",
    params(("key" = String, Path)), responses((status = 200, description = "参数值", body = ApiResponse<String>)), security(("bearer" = [])))]
async fn get_by_key(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(key): Path<String>,
) -> AppResult<Json<ApiResponse<String>>> {
    match state
        .services
        .config
        .find_by_key(&current_user, &key)
        .await?
    {
        Some(cfg) => Ok(Json(ApiResponse::success(cfg.value))),
        None => Err(ryframe_common::AppError::NotFound(format!(
            "参数 '{}' 不存在",
            key
        ))),
    }
}

/// 刷新参数缓存
///
/// 清空所有参数配置的 Redis 缓存
#[delete("/cache")]
#[perm("system:config:edit")]
#[utoipa::path(delete, path = "/api/v1/system/configs/cache", tag = "参数配置",
    responses((status = 200, description = "缓存刷新成功", body = ryframe_common::ApiEmptyResponse)), security(("bearer" = [])))]
async fn refresh_cache(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<()>>> {
    let deleted = state.services.config.clear_cache(&current_user).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "已清除 {deleted} 个缓存"
    ))))
}

/// 参数导出数据
#[derive(Debug, Serialize)]
struct ConfigExportData {
    pub name: String,
    pub key: String,
    pub value: String,
    pub remark: Option<String>,
    pub created_at: String,
}

impl ConfigExportData {
    fn excel_headers() -> Vec<(&'static str, &'static str)> {
        vec![
            ("name", "参数名称"),
            ("key", "参数键名"),
            ("value", "参数键值"),
            ("remark", "备注"),
            ("created_at", "创建时间"),
        ]
    }
}

/// 导出参数配置
#[get("/export")]
#[perm("system:config:export")]
#[utoipa::path(get, path = "/api/v1/system/configs/export", tag = "参数配置",
    params(ConfigFilterQuery),
    responses((status = 200, description = "导出配置 Excel", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")), security(("bearer" = [])))]
async fn export_configs(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<ConfigFilterQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let configs = state
        .services
        .config
        .find_all(
            &current_user,
            query.into_service_params(PageQuery::all_records()),
        )
        .await?;
    let export_data: Vec<ConfigExportData> = configs
        .into_iter()
        .map(|c| ConfigExportData {
            name: c.name,
            key: c.key,
            value: c.value,
            remark: c.remark,
            created_at: c.created_at.to_rfc3339(),
        })
        .collect();

    let bytes = ExcelExporter::export_to_bytes(
        &export_data,
        "参数配置",
        &ConfigExportData::excel_headers(),
    )?;

    excel_response(bytes, "configs.xlsx")
}
