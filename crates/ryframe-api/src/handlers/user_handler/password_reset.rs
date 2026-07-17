use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_common::{ApiResponse, AppResult};
use ryframe_macro::post;
use validator::Validate;

use crate::{
    dto::user_dto::{PasswordResetRequestDto, PasswordResetRequestResponse},
    state::AppState,
};

#[post("/{id}/password-reset-requests")]
#[perm("system:user:edit")]
#[utoipa::path(post, path = "/api/v1/system/users/{id}/password-reset-requests", tag = "用户管理",
    params(("id" = i64, Path, description = "用户ID")),
    request_body = PasswordResetRequestDto,
    responses((status = 200, description = "密码重置请求已发起", body = ApiResponse<PasswordResetRequestResponse>)),
    security(("bearer" = [])))]
pub(crate) async fn request_password_reset(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    Path(id): Path<i64>,
    Json(dto): Json<PasswordResetRequestDto>,
) -> AppResult<Json<ApiResponse<PasswordResetRequestResponse>>> {
    dto.validate()?;
    let outcome = state
        .services
        .user
        .request_password_reset(
            &current_user,
            id,
            &dto.reason,
            Some(remote_addr.ip().to_string()),
        )
        .await?;
    let response = PasswordResetRequestResponse {
        request_id: outcome.request.id.to_string(),
        reset_token: outcome.token.clone(),
        reset_url: format!(
            "/reset-password?tenant_id={}&request_id={}&token={}",
            outcome.request.tenant_id, outcome.request.id, outcome.token
        ),
        expires_at: outcome.request.expires_at.to_rfc3339(),
    };
    Ok(Json(ApiResponse::success_msg(
        "password reset request created",
        response,
    )))
}
