use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use ryframe_auth::{RequestPrincipal, jwt::Claims};
use ryframe_http::{ApiResponse, HttpAppError, HttpResult};
use ryframe_kernel::AppError;

use crate::{
    message_socket::WebSocketTicketResponse, request_locale::RequestLocale, state::AppState,
};

#[utoipa::path(
    post,
    path = "/api/v1/auth/ws-ticket",
    tag = "认证",
    responses(
        (status = 200, description = "一次性 WebSocket 票据", body = ApiResponse<WebSocketTicketResponse>),
        (status = 401, description = "未认证"),
        (status = 503, description = "Redis 不可用；显式禁用时返回 Retry-After: 60", headers(("Retry-After" = String, description = "仅 Redis 显式禁用时为 60 秒")))
    ),
    security(("bearer" = []))
)]
/// 申请仅可用于一次 WebSocket 握手的短期票据。
///
/// GET 升级请求不再携带 access token，避免令牌出现在 URL、代理日志或浏览器历史中。
pub async fn websocket_ticket(
    State(state): State<AppState>,
    current_user: RequestPrincipal,
    Extension(claims): Extension<Claims>,
    Extension(request_locale): Extension<RequestLocale>,
) -> HttpResult<Response> {
    let grant = match state
        .services
        .websocket_ticket
        .issue(&current_user, &claims, request_locale.0.as_str())
        .await
    {
        Ok(grant) => grant,
        Err(error @ AppError::ServiceUnavailable(_))
            if state.websocket_ticket_redis_is_explicitly_disabled() =>
        {
            ryframe_middleware::metrics::record_ws_ticket("backend_error");
            let mut response = HttpAppError::from(error).into_response();
            ryframe_http::mark_expected_service_unavailable(&mut response);
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
            return Ok(response);
        }
        Err(error) => return Err(error.into()),
    };
    ryframe_middleware::metrics::record_ws_ticket("issued");
    Ok(Json(ApiResponse::success(WebSocketTicketResponse {
        ticket: grant.ticket,
        expires_in: grant.expires_in,
    }))
    .into_response())
}
