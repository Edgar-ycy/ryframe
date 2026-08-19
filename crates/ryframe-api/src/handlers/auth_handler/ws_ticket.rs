use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderName, HeaderValue, header},
    response::{IntoResponse, Response},
};
use ryframe_auth::jwt::Claims;
use ryframe_http::{ApiResponse, HttpAppError, HttpResult};
use ryframe_kernel::AppError;

use crate::{
    RequestPrincipal, message_socket::WebSocketTicketResponse, request_locale::RequestLocale,
    state::AppState,
};

/// 实时通道不可用时供客户端识别受控降级的响应头。
///
/// 该头与 `Retry-After` 一起使用：客户端应停止短周期 WebSocket 重连，并继续
/// 通过收件箱 REST 接口补拉消息。值仅表示本次实时通道不可用，不影响已有的
/// HTTP 授权纪元回退机制。
const REALTIME_STATUS_HEADER: &str = "x-ryframe-realtime";
const REALTIME_STATUS_UNAVAILABLE: &str = "unavailable";
const REALTIME_UNAVAILABLE_RETRY_AFTER_SECONDS: &str = "60";

#[utoipa::path(
    post,
    path = "/api/v1/auth/ws-ticket",
    tag = "认证",
    responses(
        (status = 200, description = "一次性 WebSocket 票据", body = ApiResponse<WebSocketTicketResponse>),
        (status = 401, description = "未认证"),
        (status = 503, description = "实时消息通道不可用；客户端应回退到收件箱轮询", headers(
            ("Retry-After" = String, description = "再次申请票据前至少等待 60 秒"),
            ("X-RyFrame-Realtime" = String, description = "固定值 unavailable，标识受控的实时通道降级")
        ))
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
        Err(error @ AppError::ServiceUnavailable(_)) => {
            ryframe_adapters::metrics::record_ws_ticket("backend_error");
            return Ok(websocket_ticket_unavailable_response(
                error,
                state.websocket_ticket_is_expected_unavailable(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    ryframe_adapters::metrics::record_ws_ticket("issued");
    Ok(Json(ApiResponse::success(WebSocketTicketResponse {
        ticket: grant.ticket,
        expires_in: grant.expires_in,
    }))
    .into_response())
}

fn websocket_ticket_unavailable_response(error: AppError, expected_unavailable: bool) -> Response {
    let mut response = HttpAppError::from(error).into_response();
    if expected_unavailable {
        // Redis optional 模式启动期降级和显式关闭消息中心都是已知状态。实际故障
        // 已在拥有依赖状态的启动或健康检查边界记录，不能让每次浏览器重试都升级
        // 成请求 ERROR。
        ryframe_http::mark_expected_service_unavailable(&mut response);
    }
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_static(REALTIME_UNAVAILABLE_RETRY_AFTER_SECONDS),
    );
    response.headers_mut().insert(
        HeaderName::from_static(REALTIME_STATUS_HEADER),
        HeaderValue::from_static(REALTIME_STATUS_UNAVAILABLE),
    );
    response
}
