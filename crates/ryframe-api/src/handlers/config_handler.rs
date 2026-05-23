use crate::dto::config_dto::{CreateConfigDto, UpdateConfigDto};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::{delete, get},
};
use ryframe_common::{ApiResponse, AppResult};
use ryframe_core::PageQuery;
use ryframe_service::system::ConfigVo;
use serde::Serialize;
use validator::Validate;

use super::auth_handler::AppState;

pub fn config_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/list", get(list))
        .route("/listNoPage", get(list_no_page))
        .route("/export", get(export_configs))
        .route("/refreshCache", delete(refresh_cache))
        .route("/configKey/{key}", get(get_by_key))
        .route("/{id}", get(detail).put(update).delete(remove))
        .with_state(state)
}

/// 参数配置列表
#[utoipa::path(get, path = "/api/v1/system/configs", tag = "参数配置",
    responses((status = 200, description = "配置列表")), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    Query(query): Query<PageQuery>,
) -> AppResult<Json<ryframe_common::ApiPageResponse<ConfigVo>>> {
    state
        .config_service
        .find_by_page(&state.db, query)
        .await
        .map(|p| Json(p.to_page_response("查询成功")))
}

/// 参数配置列表不分页查询（返回全部数据）
async fn list_no_page(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<Vec<ConfigVo>>>> {
    state
        .config_service
        .find_all(&state.db)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

async fn detail(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Json<ApiResponse<ConfigVo>>> {
    match state.config_service.find_by_id(&state.db, id).await? {
        Some(cfg) => Ok(Json(ApiResponse::success(cfg))),
        None => Err(ryframe_common::AppError::NotFound("参数配置不存在".into())),
    }
}

/// 创建参数配置
#[utoipa::path(post, path = "/api/v1/system/configs", tag = "参数配置",
    request_body = CreateConfigDto, responses((status = 200, description = "创建成功")), security(("bearer" = [])))]
async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateConfigDto>,
) -> AppResult<Json<ApiResponse<ConfigVo>>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state
        .config_service
        .create(
            &state.db,
            &dto.name,
            &dto.key,
            &dto.value,
            dto.remark.as_deref(),
        )
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 更新参数配置
#[utoipa::path(put, path = "/api/v1/system/configs/{id}", tag = "参数配置",
    params(("id" = i64, Path)), request_body = UpdateConfigDto,
    responses((status = 200, description = "更新成功")), security(("bearer" = [])))]
async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateConfigDto>,
) -> AppResult<Json<ApiResponse<ConfigVo>>> {
    dto.validate()
        .map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state
        .config_service
        .update(&state.db, id, &dto.value)
        .await
        .map(|v| Json(ApiResponse::success(v)))
}

/// 删除参数配置
#[utoipa::path(delete, path = "/api/v1/system/configs/{id}", tag = "参数配置",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功")), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<ApiResponse<()>>> {
    state.config_service.delete(&state.db, id).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg("删除成功")))
}

/// 根据参数键名查询参数值
#[utoipa::path(get, path = "/api/v1/system/configs/configKey/{key}", tag = "参数配置",
    params(("key" = String, Path)), responses((status = 200, description = "参数值")), security(("bearer" = [])))]
async fn get_by_key(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> AppResult<Json<ApiResponse<String>>> {
    match state.config_service.find_by_key(&state.db, &key).await? {
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
async fn refresh_cache(State(state): State<AppState>) -> AppResult<Json<ApiResponse<()>>> {
    if let Some(ref redis) = state.redis
        && let Ok(keys) = redis.keys("sys_config:key:*").await
    {
        for key in &keys {
            let _ = redis.del(key).await;
        }
        return Ok(Json(ApiResponse::success_no_data_with_msg(format!(
            "已清除 {} 个缓存",
            keys.len()
        ))));
    }
    Ok(Json(ApiResponse::success_no_data_with_msg("缓存刷新成功")))
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
async fn export_configs(State(state): State<AppState>) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;

    let configs = state.config_service.find_all(&state.db).await?;
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

    let response = axum::response::Response::builder()
        .status(200)
        .header(
            "Content-Type",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        )
        .header("Content-Disposition", "attachment; filename=configs.xlsx")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ryframe_common::AppError::Internal(format!("构建响应失败: {}", e)))?;

    Ok(response)
}
