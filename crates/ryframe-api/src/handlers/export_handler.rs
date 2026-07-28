use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiResponse, AppError, AppResult};
use ryframe_macro::{get, post, route};
use ryframe_service::system::{ExportJobVo, RequestExportCommand};

use crate::{dto::export_dto::CancelExportJobDto, handler_utils::excel_response, state::AppState};

/// 导出任务查询、取消与下载路由。
pub fn export_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(detail))
        .merge(route!(cancel))
        .merge(route!(download))
        .with_state(state)
}

/// 查询当前用户可以访问的最近导出任务。
#[get("/")]
#[utoipa::path(get, path = "/api/v1/common/jobs", tag = "导出任务",
    responses((status = 200, description = "导出任务列表", body = ApiResponse<Vec<ExportJobVo>>)),
    security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> AppResult<Json<ApiResponse<Vec<ExportJobVo>>>> {
    state
        .services
        .export
        .list_for_requester(&current_user)
        .await
        .map_err(AppError::from)
        .map(ApiResponse::success)
        .map(Json)
}

/// 查询当前用户自己的导出任务。
#[get("/{id}")]
#[utoipa::path(get, path = "/api/v1/common/jobs/{id}", tag = "导出任务",
    params(("id" = String, Path, description = "导出任务 ID")),
    responses((status = 200, description = "导出任务详情", body = ApiResponse<ExportJobVo>)),
    security(("bearer" = [])))]
async fn detail(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> AppResult<Json<ApiResponse<ExportJobVo>>> {
    state
        .services
        .export
        .find_for_requester(&current_user, parse_export_id(&id)?)
        .await
        .map_err(AppError::from)
        .map(ApiResponse::success)
        .map(Json)
}

/// 取消当前用户尚未完成的导出任务。
#[post("/{id}/cancel")]
#[utoipa::path(post, path = "/api/v1/common/jobs/{id}/cancel", tag = "导出任务",
    params(("id" = String, Path, description = "导出任务 ID")),
    request_body = CancelExportJobDto,
    responses((status = 200, description = "导出任务已取消", body = ApiResponse<ExportJobVo>)),
    security(("bearer" = [])))]
async fn cancel(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Json(_request): Json<CancelExportJobDto>,
) -> AppResult<Json<ApiResponse<ExportJobVo>>> {
    state
        .services
        .export
        .cancel_for_requester(&current_user, parse_export_id(&id)?)
        .await
        .map_err(AppError::from)
        .map(|job| ApiResponse::success_msg("导出任务已取消", job))
        .map(Json)
}

/// 下载当前用户尚未过期的导出结果。
#[get("/{id}/download")]
#[utoipa::path(get, path = "/api/v1/common/jobs/{id}/download", tag = "导出任务",
    params(("id" = String, Path, description = "导出任务 ID")),
    responses((status = 200, description = "导出文件", body = Vec<u8>, content_type = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")),
    security(("bearer" = [])))]
async fn download(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> AppResult<axum::response::Response> {
    let location = state
        .services
        .export
        .download_location_for_requester(&current_user, parse_export_id(&id)?)
        .await
        .map_err(AppError::from)?;
    let (bytes, filename) = state
        .services
        .file
        .download(&current_user, &location.bucket, &location.path)
        .await
        .map_err(AppError::from)?;
    excel_response(bytes, &filename)
}

fn parse_export_id(value: &str) -> AppResult<i64> {
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation("导出任务 ID 必须是正整数".into()))
}

/// 创建导出任务前必须显式给出幂等键，实际重放语义由系统路由的幂等中间件统一处理。
pub(crate) async fn request_export(
    state: AppState,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    resource: &str,
    permission_code: &str,
    request_params: serde_json::Value,
) -> AppResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    require_idempotency_key(&headers)?;
    let export = state
        .services
        .export
        .request(
            &current_user,
            RequestExportCommand {
                resource: resource.into(),
                permission_code: permission_code.into(),
                request_params,
            },
        )
        .await
        .map_err(AppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success_msg("导出任务已创建", export)),
    ))
}

fn require_idempotency_key(headers: &HeaderMap) -> AppResult<()> {
    let valid = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| !value.trim().is_empty());
    valid
        .then_some(())
        .ok_or_else(|| AppError::Validation("导出任务必须提供 Idempotency-Key 请求头".into()))
}
