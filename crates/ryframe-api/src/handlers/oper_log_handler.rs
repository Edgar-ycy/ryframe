use crate::RequestPrincipal;
use crate::dto::oper_log_dto::OperLogPageQuery;
use crate::dto::public_dto::{ExportJobVo, OperLogVo};
use crate::http::{ApiPageResponse, ApiResponse, HttpResult};
use crate::state::AppState;
use crate::{dto::export_dto::OperLogExportRequestDto, handlers::export_handler::request_export};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_macro::{get, post, route};

pub fn oper_log_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(request_oper_log_export))
        .with_state(state)
}

/// 操作日志列表
#[get("/")]
#[perm("system:operlog:list")]
#[utoipa::path(get, path = "/api/v1/system/operlogs", tag = "操作日志",
    params(OperLogPageQuery),
    responses((status = 200, description = "日志列表", body = ApiPageResponse<OperLogVo>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<OperLogPageQuery>,
) -> HttpResult<Json<ApiPageResponse<OperLogVo>>> {
    state
        .services
        .oper_log
        .find_by_page(&current_user, query.into_service_query(state.pagination)?)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(|p| {
            Json(ApiPageResponse::page(
                p.records.into_iter().map(OperLogVo::from).collect(),
                p.total,
                p.page,
                p.page_size,
                state.pagination.max_page_size(),
            ))
        })
}

/// 创建操作日志异步导出任务。
#[post("/exports")]
#[perm("system:operlog:export")]
#[utoipa::path(post, path = "/api/v1/system/operlogs/exports", tag = "操作日志",
    params(("Idempotency-Key" = String, Header, description = "幂等键")), request_body = OperLogExportRequestDto,
    responses((status = 202, description = "操作日志导出任务已创建", body = ApiResponse<ExportJobVo>)), security(("bearer" = [])))]
async fn request_oper_log_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<OperLogExportRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    let (selection, confirm_all) = request.into_selection()?;
    request_export(
        state,
        current_user,
        headers,
        "system:operlog:export",
        selection,
        confirm_all,
    )
    .await
}
