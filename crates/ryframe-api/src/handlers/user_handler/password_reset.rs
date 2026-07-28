use std::net::SocketAddr;

use axum::{
    Json,
    extract::{ConnectInfo, Path, State},
};
use ryframe_auth::RequestPrincipal;
use ryframe_http::{ApiResponse, AppResult};
use ryframe_macro::post;
use ryframe_utils::ip::ClientIp;
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
    client_ip: Option<axum::Extension<ClientIp>>,
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
    Ok(Json(ApiResponse::success_msg(
        "password reset request created",
        response,
    )))
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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};

    use axum::Extension;
    use ryframe_utils::ip::ClientIp;

    use super::request_audit_ip;

    #[test]
    fn password_reset_audit_prefers_trusted_client_ip() {
        let client_ip = "203.0.113.7".parse::<IpAddr>().expect("有效的客户端 IP");
        let proxy_addr = "10.0.0.8:8080"
            .parse::<SocketAddr>()
            .expect("有效的代理地址");

        assert_eq!(
            request_audit_ip(Some(Extension(ClientIp(client_ip))), proxy_addr),
            "203.0.113.7"
        );
        assert_eq!(request_audit_ip(None, proxy_addr), "10.0.0.8");
    }
}
