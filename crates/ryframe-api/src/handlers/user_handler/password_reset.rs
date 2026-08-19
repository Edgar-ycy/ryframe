use std::net::SocketAddr;

use crate::ClientIp;
use crate::RequestPrincipal;
use crate::http::{ApiResponse, HttpResult};
use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
};
use ryframe_macro::post;
use validator::Validate;

use crate::{
    dto::user_dto::{PasswordResetRequestDto, PasswordResetRequestResponse},
    state::AppState,
};

#[post("/{id}/password-reset-requests")]
#[perm("system:user:edit")]
#[utoipa::path(post, path = "/api/v1/system/users/{id}/password-reset-requests", tag = "用户管理",
    params(("id" = String, Path, description = "用户ID")),
    request_body = PasswordResetRequestDto,
    responses((status = 200, description = "密码重置请求已发起", body = ApiResponse<PasswordResetRequestResponse>)),
    security(("bearer" = [])))]
pub(crate) async fn request_password_reset(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    client_ip: Option<axum::Extension<ClientIp>>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    Path(id): Path<i64>,
    Json(dto): Json<PasswordResetRequestDto>,
) -> HttpResult<Json<ApiResponse<PasswordResetRequestResponse>>> {
    dto.validate()?;
    let outcome = state
        .services
        .user
        .request_password_reset(
            &current_user,
            id,
            &dto.reason,
            Some(request_audit_ip(client_ip, remote_addr)),
        )
        .await?;
    let response = PasswordResetRequestResponse {
        request_id: outcome.request.id.to_string(),
        reset_url: format!(
            "/reset-password#tenant_id={}&request_id={}&token={}",
            outcome.request.tenant_id, outcome.request.id, outcome.token
        ),
        expires_at: outcome.request.expires_at.to_rfc3339(),
    };
    Ok(Json(ApiResponse::success(response)))
}

fn request_audit_ip(
    client_ip: Option<axum::Extension<ClientIp>>,
    remote_addr: SocketAddr,
) -> String {
    client_ip.map_or_else(
        || remote_addr.ip().to_string(),
        |axum::Extension(client_ip)| client_ip.0.to_string(),
    )
}
