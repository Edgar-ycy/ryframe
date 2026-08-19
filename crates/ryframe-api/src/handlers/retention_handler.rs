use crate::RequestPrincipal;
use crate::http::{ApiPageResponse, ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
};
use ryframe_macro::{get, post, route};

use crate::{
    dto::{
        empty_dto::EmptyRequestDto,
        public_dto::{DataRetentionOverview, DataRetentionPreview, DataRetentionRunVo},
        retention_dto::RetentionRunPageQuery,
    },
    handler_utils::idempotency_key_hash,
    state::AppState,
};

pub fn retention_router(state: AppState) -> Router {
    Router::new()
        .merge(route!(overview))
        .merge(route!(preview))
        .merge(route!(run))
        .merge(route!(runs))
        .with_state(state)
}

#[get("/retention")]
#[perm("monitor:retention:list")]
#[utoipa::path(get, path = "/api/v1/monitor/retention", tag = "数据保留",
    responses((status = 200, description = "当前有效保留策略", body = ApiResponse<DataRetentionOverview>)),
    security(("bearer" = [])))]
async fn overview(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
) -> HttpResult<Json<ApiResponse<DataRetentionOverview>>> {
    state
        .services
        .data_retention
        .overview(&current_user)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(DataRetentionOverview::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[post("/retention/preview")]
#[perm("monitor:retention:list")]
#[utoipa::path(post, path = "/api/v1/monitor/retention/preview", tag = "数据保留",
    request_body = EmptyRequestDto,
    responses((status = 200, description = "预计可清理数量", body = ApiResponse<DataRetentionPreview>)),
    security(("bearer" = [])))]
async fn preview(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Json(_request): Json<EmptyRequestDto>,
) -> HttpResult<Json<ApiResponse<DataRetentionPreview>>> {
    state
        .services
        .data_retention
        .preview(&current_user)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(DataRetentionPreview::from)
        .map(ApiResponse::success)
        .map(Json)
}

#[post("/retention/run")]
#[perm("monitor:retention:run")]
#[utoipa::path(post, path = "/api/v1/monitor/retention/run", tag = "数据保留",
    params(("Idempotency-Key" = String, Header, description = "人工清理幂等键")),
    request_body = EmptyRequestDto,
    responses((status = 202, description = "清理任务已入队", body = ApiResponse<DataRetentionRunVo>)),
    security(("bearer" = [])))]
async fn run(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    headers: HeaderMap,
    Json(_request): Json<EmptyRequestDto>,
) -> HttpResult<(StatusCode, Json<ApiResponse<DataRetentionRunVo>>)> {
    let hash = idempotency_key_hash(&headers)?;
    let run = state
        .services
        .data_retention
        .enqueue_manual(&current_user, &hash)
        .await
        .map_err(crate::http::HttpAppError::from)?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::success(run.into()))))
}

#[get("/retention/runs")]
#[perm("monitor:retention:list")]
#[utoipa::path(get, path = "/api/v1/monitor/retention/runs", tag = "数据保留",
    params(RetentionRunPageQuery),
    responses((status = 200, description = "数据保留运行记录", body = ApiPageResponse<DataRetentionRunVo>)),
    security(("bearer" = [])))]
async fn runs(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Query(query): Query<RetentionRunPageQuery>,
) -> HttpResult<Json<ApiPageResponse<DataRetentionRunVo>>> {
    let page = state
        .services
        .data_retention
        .list_runs(&current_user, query.into_page(&state.config.pagination)?)
        .await
        .map_err(crate::http::HttpAppError::from)?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map(Into::into).collect(),
        page.total,
        page.page,
        page.page_size,
        state.config.pagination.max_page_size,
    )))
}
