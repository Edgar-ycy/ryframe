use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::Response,
};
use ryframe_utils::ip::{ClientIp, TrustedProxySet};

/// 尽早解析唯一可信的客户端地址，并使其可供日志、限流、认证和审计中间件使用。
pub async fn trusted_client_ip_middleware(
    State(trusted_proxies): State<TrustedProxySet>,
    mut request: Request,
    next: Next,
) -> Response {
    let client_ip = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| trusted_proxies.client_ip(request.headers(), peer.ip()))
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    request.extensions_mut().insert(ClientIp(client_ip));
    next.run(request).await
}
