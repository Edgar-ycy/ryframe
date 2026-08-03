use crate::dto::login_log_dto::LoginLogPageQuery;
use crate::dto::public_dto::{ExportJobVo, LoginInfoVo};
use crate::state::AppState;
use crate::{dto::export_dto::ExportRequestDto, handlers::export_handler::request_export};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiPageResponse, ApiResponse, HttpResult};
use ryframe_macro::{get, post, route};

pub fn login_log_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(list))
        .merge(route!(request_login_log_export))
        .with_state(state)
}

/// 登录日志列表
#[get("/")]
#[perm("system:logininfor:list")]
#[utoipa::path(get, path = "/api/v1/system/loginlogs", tag = "登录日志",
    params(LoginLogPageQuery),
    responses((status = 200, description = "日志列表", body = ApiPageResponse<LoginInfoVo>)), security(("bearer" = [])))]
async fn list(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<LoginLogPageQuery>,
) -> HttpResult<Json<ApiPageResponse<LoginInfoVo>>> {
    state
        .services
        .login_info
        .find_by_page(
            &current_user,
            query.into_service_query(&state.config.pagination)?,
        )
        .await
        .map_err(ryframe_http::HttpAppError::from)
        .map(|p| {
            Json(ApiPageResponse::new(
                p.records.into_iter().map(LoginInfoVo::from).collect(),
                p.total,
                p.page,
                p.page_size,
                state.config.pagination.max_page_size,
                "查询成功",
            ))
        })
}

/// 创建登录日志异步导出任务。
#[post("/exports")]
#[perm("system:logininfor:export")]
#[utoipa::path(post, path = "/api/v1/system/loginlogs/exports", tag = "登录日志",
    params(("Idempotency-Key" = String, Header, description = "幂等键")), request_body = ExportRequestDto,
    responses((status = 202, description = "登录日志导出任务已创建", body = ApiResponse<ExportJobVo>)), security(("bearer" = [])))]
async fn request_login_log_export(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(request): Json<ExportRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<ExportJobVo>>)> {
    request_export(
        state,
        current_user,
        headers,
        "loginlogs",
        "system:logininfor:export",
        request.0,
    )
    .await
}
