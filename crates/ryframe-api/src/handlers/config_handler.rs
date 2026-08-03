use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_macro::{delete, get, post, put, route};
use validator::Validate;

use crate::dto::config_dto::{ConfigListQuery, CreateConfigDto, UpdateConfigDto};
use crate::dto::public_dto::{ConfigVo, ExportJobVo};
use crate::state::AppState;
use crate::{dto::export_dto::ExportRequestDto, handlers::export_handler::request_export};

pub fn config_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(request_config_export))
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
) -> HttpResult<Json<ryframe_http::ApiPageResponse<ConfigVo>>> {
    state
        .services
        .config
        .find_by_page(
            &current_user,
            query.into_service_params(&state.config.pagination)?,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|p| {
            Json(ApiPageResponse::new(
                p.records.into_iter().map(ConfigVo::from).collect(),
                p.total,
                p.page,
                p.page_size,
                state.config.pagination.max_page_size,
                "查询成功",
            ))
        })
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
) -> HttpResult<Json<ApiResponse<ConfigVo>>> {
    match state.services.config.find_by_id(&current_user, id).await? {
        Some(cfg) => Ok(Json(ApiResponse::success(cfg.into()))),
        None => Err(ryframe_kernel::AppError::NotFound("参数配置不存在".into()).into()),
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
) -> HttpResult<Json<ApiResponse<ConfigVo>>> {
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
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
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
) -> HttpResult<Json<ApiResponse<ConfigVo>>> {
    dto.validate()?;
    state
        .services
        .config
        .update(&current_user, id, &dto.value)
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|value| Json(ApiResponse::success(value.into())))
}

/// 删除参数配置
#[delete("/{id}")]
#[perm("system:config:remove")]
#[utoipa::path(delete, path = "/api/v1/system/configs/{id}", tag = "参数配置",
    params(("id" = i64, Path)), responses((status = 200, description = "删除成功", body = ryframe_http::ApiEmptyResponse)), security(("bearer" = [])))]
async fn remove(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<()>>> {
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
) -> HttpResult<Json<ApiResponse<String>>> {
    match state
        .services
        .config
        .find_by_key(&current_user, &key)
        .await?
    {
        Some(cfg) => Ok(Json(ApiResponse::success(cfg.value))),
        None => Err(ryframe_kernel::AppError::NotFound(format!("参数 '{}' 不存在", key)).into()),
    }
}

/// 刷新参数缓存
///
/// 清空所有参数配置的 Redis 缓存
#[delete("/cache")]
#[perm("system:config:edit")]
#[utoipa::path(delete, path = "/api/v1/system/configs/cache", tag = "参数配置",
    responses((status = 200, description = "缓存刷新成功", body = ryframe_http::ApiEmptyResponse)), security(("bearer" = [])))]
async fn refresh_cache(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<()>>> {
    let deleted = state.services.config.clear_cache(&current_user).await?;
    Ok(Json(ApiResponse::success_no_data_with_msg(format!(
        "已清除 {deleted} 个缓存"
    ))))
}

/// 创建参数配置异步导出任务。
#[post("/exports")]
#[perm("system:config:export")]
#[utoipa::path(post, path = "/api/v1/system/configs/exports", tag = "参数配置",
    params(("Idempotency-Key" = String, Header, description = "幂等键")), request_body = ExportRequestDto,
    responses((status = 202, description = "参数配置导出任务已创建", body = ApiResponse<ExportJobVo>)), security(("bearer" = [])))]
async fn request_config_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<ExportRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    request_export(
        state,
        current_user,
        headers,
        "configs",
        "system:config:export",
        request.0,
    )
    .await
}
