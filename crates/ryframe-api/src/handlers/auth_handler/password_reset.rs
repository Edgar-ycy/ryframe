use axum::{Json, extract::State};
use ryframe_http::{ApiResponse, HttpResult};
use ryframe_kernel::AppError;
use validator::Validate;

use crate::{dto::auth_dto::CompletePasswordResetRequest, state::AppState};

#[utoipa::path(
    post,
    path = "/api/v1/auth/password-reset/complete",
    tag = "认证",
    request_body = CompletePasswordResetRequest,
    responses(
        (status = 200, description = "密码已重置", body = ryframe_http::ApiEmptyResponse),
        (status = 400, description = "参数校验失败"),
        (status = 401, description = "重置令牌无效")
    )
)]
pub async fn complete_password_reset(
    State(state): State<AppState>,
    Json(req): Json<CompletePasswordResetRequest>,
) -> HttpResult<Json<ApiResponse<()>>> {
    req.validate()?;
    let request_id = req
        .request_id
        .parse::<i64>()
        .map_err(|_| AppError::Validation("无效的重置请求ID".into()))?;
    state
        .services
        .user
        .complete_password_reset_request(&req.tenant_id, request_id, &req.token, &req.new_password)
        .await?;
    Ok(Json(ApiResponse::success_no_data()))
}
