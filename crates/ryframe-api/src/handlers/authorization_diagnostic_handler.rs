use crate::RequestPrincipal;
use crate::http::{ApiResponse, HttpResult};
use axum::{
    Json, Router,
    extract::{Path, State},
};
use ryframe_macro::{get, route};

use crate::{dto::public_dto::AuthorizationDiagnosticVo, state::AppState};

pub fn authorization_diagnostic_router(state: AppState) -> Router {
    Router::new().merge(route!(diagnose_user)).with_state(state)
}

#[get("/users/{id}")]
#[perm("system:authorization-diagnostic:list")]
#[utoipa::path(
    get,
    path = "/api/v1/system/authorization-diagnostics/users/{id}",
    tag = "权限诊断",
    params(("id" = String, Path, description = "目标用户ID")),
    responses((status = 200, description = "主库授权诊断结果", body = ApiResponse<AuthorizationDiagnosticVo>)),
    security(("bearer" = []))
)]
pub(crate) async fn diagnose_user(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Path(id): Path<i64>,
) -> HttpResult<Json<ApiResponse<AuthorizationDiagnosticVo>>> {
    state
        .services
        .authorization_diagnostic
        .diagnose(&current_user, id)
        .await
        .map_err(crate::http::HttpAppError::from)
        .map(AuthorizationDiagnosticVo::from)
        .map(ApiResponse::success)
        .map(Json)
}
