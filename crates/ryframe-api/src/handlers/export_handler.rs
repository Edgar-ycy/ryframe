use crate::RequestPrincipal;
use crate::http::{ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_application::system::{ExportSelection, RequestExportCommand};
use ryframe_kernel::AppError;
use ryframe_macro::{get, post, route};

use crate::{
    dto::{
        export_dto::{CancelExportJobDto, MarkExportNotificationsReadDto},
        export_dto::{DeleteExportJobsDto, ExportDeletionAcceptedDto},
        public_dto::ExportJobVo,
    },
    handler_utils::excel_response,
    state::AppState,
};

/// 导出任务查询、取消与下载路由。
pub fn export_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(unread_notification_count))
        .merge(route!(delete_records))
        .merge(route!(detail))
        .merge(route!(cancel))
        .merge(route!(download))
        .with_state(state)
}

/// 导出通知已读路由，由组合层显式装配独立审计事务。
pub fn notification_read_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(mark_notifications_read))
        .with_state(state)
}

/// 永久删除当前用户的一批终态导出记录及结果文件。
#[post("/deletions")]
#[utoipa::path(post, path = "/api/v1/common/jobs/deletions", tag = "导出任务",
    params(("Idempotency-Key" = String, Header, description = "必填幂等键")),
    request_body = DeleteExportJobsDto,
    responses(
        (status = 202, description = "导出记录删除已受理", body = ApiResponse<ExportDeletionAcceptedDto>),
        (status = 404, description = "任一任务不存在或不属于当前用户"),
        (status = 409, description = "任一任务仍在排队、执行或持有活动租约")
    ),
    security(("bearer" = [])))]
async fn delete_records(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<DeleteExportJobsDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportDeletionAcceptedDto>>)> {
    require_idempotency_key(&headers)?;
    let accepted = state
        .services
        .export
        .delete_for_requester(&current_user, request.into_ids()?)
        .await
        .map_err(crate::http::HttpAppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(accepted.into())),
    ))
}

/// 查询当前用户尚未查看的导出完成或失败通知数量。
#[get("/notifications/unread-count")]
#[utoipa::path(get, path = "/api/v1/common/jobs/notifications/unread-count", tag = "导出任务",
    responses((status = 200, description = "未读导出通知数量", body = ApiResponse<u64>)),
    security(("bearer" = [])))]
async fn unread_notification_count(
    state: State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<u64>>> {
    state
        .0
        .services
        .export
        .unread_notification_count(&current_user)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(ApiResponse::success)
        .map(Json)
}

/// 将当前用户已经实际看到的导出完成或失败通知标记为已查看。
#[post("/notifications/read")]
#[utoipa::path(post, path = "/api/v1/common/jobs/notifications/read", tag = "导出任务",
    request_body = MarkExportNotificationsReadDto,
    responses((status = 200, description = "已查看的导出通知数量", body = ApiResponse<u64>)),
    security(("bearer" = [])))]
async fn mark_notifications_read(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(request): Json<MarkExportNotificationsReadDto>,
) -> HttpResult<Json<ApiResponse<u64>>> {
    if request.ids.is_empty() || request.ids.len() > 100 {
        return Err(AppError::Validation("导出通知 ID 数量必须介于 1 和 100 之间".into()).into());
    }
    let mut ids = request
        .ids
        .iter()
        .map(|id| parse_export_id(id))
        .collect::<HttpResult<Vec<_>>>()?;
    ids.sort_unstable();
    ids.dedup();
    state
        .services
        .export
        .mark_notifications_read(&current_user, &ids)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(ApiResponse::success)
        .map(Json)
}

/// 查询当前用户可以访问的最近导出任务。
#[get("/")]
#[utoipa::path(get, path = "/api/v1/common/jobs", tag = "导出任务",
    responses((status = 200, description = "导出任务列表", body = ApiResponse<Vec<ExportJobVo>>)),
    security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<Vec<ExportJobVo>>>> {
    state
        .services
        .export
        .list_for_requester(&current_user)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(|jobs| jobs.into_iter().map(ExportJobVo::from).collect())
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
) -> HttpResult<Json<ApiResponse<ExportJobVo>>> {
    state
        .services
        .export
        .find_for_requester(&current_user, parse_export_id(&id)?)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(ExportJobVo::from)
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
) -> HttpResult<Json<ApiResponse<ExportJobVo>>> {
    state
        .services
        .export
        .cancel_for_requester(&current_user, parse_export_id(&id)?)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(|job| ApiResponse::success(job.into()))
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
) -> HttpResult<axum::response::Response> {
    let location = state
        .services
        .export
        .download_location_for_requester(&current_user, parse_export_id(&id)?)
        .await
        .map_err(crate::http::HttpAppError::from)?;
    state
        .services
        .file
        .download(&current_user, &location.bucket, &location.path)
        .await
        .map_err(crate::http::HttpAppError::from)
        .and_then(|file| {
            // 导出结果只允许以受控的 Excel 类型返回，不信任通用文件元数据覆盖响应类型。
            excel_response(file.data, &file.original_name)
        })
}

fn parse_export_id(value: &str) -> HttpResult<i64> {
    Ok(value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Validation("导出任务 ID 必须是正整数".into()))?)
}

/// 创建导出任务前必须显式给出幂等键，实际重放语义由系统路由的幂等中间件统一处理。
pub(crate) async fn request_export(
    state: AppState,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    permission_code: &str,
    selection: ExportSelection,
    confirm_all: bool,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    require_idempotency_key(&headers)?;
    let export = state
        .services
        .export
        .request(
            &current_user,
            RequestExportCommand {
                permission_code: permission_code.into(),
                selection,
                confirm_all,
            },
        )
        .await
        .map_err(crate::http::HttpAppError::from)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(export.into())),
    ))
}

pub fn require_idempotency_key(headers: &HeaderMap) -> HttpResult<()> {
    let valid = headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        });
    Ok(valid
        .then_some(())
        .ok_or_else(|| AppError::Validation("导出任务必须提供 Idempotency-Key 请求头".into()))?)
}
