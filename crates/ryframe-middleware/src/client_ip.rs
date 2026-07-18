use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::Response,
};
use ryframe_common::utils::ip::{ClientIp, TrustedProxySet};

/// Resolve a single trusted client address early and make it available to
/// logging, rate limiting, authentication, and audit middleware.
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
