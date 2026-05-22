use axum::{extract::{Query, State}, routing::get, Json, Router};
use ryframe_common::AppResult;
use ryframe_core::PageResult;
use ryframe_service::system::LoginInfoVo;

use crate::dto::login_log_dto::LoginLogPageQuery;
use super::auth_handler::AppState;

pub fn login_log_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list))
        .route("/clean", axum::routing::delete(clean))
        .with_state(state)
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<LoginLogPageQuery>,
) -> AppResult<Json<PageResult<LoginInfoVo>>> {
    state
        .login_info_service
        .find_by_page(
            &state.db,
            ryframe_core::PageQuery { page: query.page, page_size: query.page_size },
            query.user_name.as_deref(),
            query.status,
            query.begin_time.as_deref(),
            query.end_time.as_deref(),
        )
        .await
        .map(Json)
}

async fn clean(State(state): State<AppState>) -> AppResult<Json<serde_json::Value>> {
    let count = state.login_info_service.clean(&state.db).await?;
    Ok(Json(serde_json::json!({"message": format!("成功清空 {} 条登录日志", count)})))
}