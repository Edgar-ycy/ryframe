use axum::{extract::{Query, State}, routing::get, Json, Router};
use ryframe_common::AppResult;
use ryframe_core::PageResult;
use ryframe_service::system::OperLogVo;

use crate::dto::oper_log_dto::OperLogPageQuery;
use super::auth_handler::AppState;

pub fn oper_log_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list))
        .route("/clean", axum::routing::delete(clean))
        .with_state(state)
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<OperLogPageQuery>,
) -> AppResult<Json<PageResult<OperLogVo>>> {
    state
        .oper_log_service
        .find_by_page(
            &state.db,
            ryframe_core::PageQuery { page: query.page, page_size: query.page_size },
            query.oper_name.as_deref(),
            query.status,
            query.begin_time.as_deref(),
            query.end_time.as_deref(),
        )
        .await
        .map(Json)
}

async fn clean(State(state): State<AppState>) -> AppResult<Json<serde_json::Value>> {
    let count = state.oper_log_service.clean(&state.db).await?;
    Ok(Json(serde_json::json!({"message": format!("成功清空 {} 条操作日志", count)})))
}