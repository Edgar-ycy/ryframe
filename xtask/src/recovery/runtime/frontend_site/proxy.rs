//! 把当前恢复代次的同源 API 请求转发给唯一登记的回环进程。
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    time::Duration,
};

use axum::{
    body::Body,
    extract::Request,
    http::{Method, StatusCode, Uri},
    response::Response,
};
use hyper::{client::conn::http1, upgrade::OnUpgrade};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::{net::TcpStream, time::timeout};

use crate::{Result, recovery::fixture_control::services::lifecycle::readonly::process};

#[path = "proxy/headers.rs"]
mod headers;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/runtime/frontend_site_proxy.rs"]
mod tests;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const UPGRADE_TIMEOUT: Duration = Duration::from_secs(15);
const API: &str = "/api/v1";
const WEBSOCKET: &str = "/api/v1/ws";

pub(super) struct Upstream {
    address: SocketAddrV4,
    identity: Value,
}

impl Upstream {
    pub(super) fn new(address: SocketAddrV4, identity: Value) -> Result<Self> {
        if *address.ip() != Ipv4Addr::LOCALHOST || address.port() == 0 {
            return Err("恢复 API 上游必须绑定明确的 IPv4 回环端口".into());
        }
        require_listener(address, &identity)?;
        Ok(Self { address, identity })
    }

    pub(super) async fn forward(&self, request: Request) -> Response {
        match self.exchange(request).await {
            Ok(response) => response,
            Err(status) => {
                let mut response = Response::new(Body::empty());
                *response.status_mut() = status;
                response
            }
        }
    }

    async fn exchange(&self, mut request: Request) -> ProxyResult<Response> {
        validate_target(request.method(), request.uri())?;
        let upgrade = headers::request(&mut request)?;
        let stream = self.connect().await?;
        relay(request, upgrade, stream).await
    }

    async fn connect(&self) -> ProxyResult<TcpStream> {
        timeout(CONNECT_TIMEOUT, async {
            let stream = TcpStream::connect(self.address)
                .await
                .map_err(|_| StatusCode::BAD_GATEWAY)?;
            let stream = stream.into_std().map_err(|_| StatusCode::BAD_GATEWAY)?;
            let identity = self.identity.clone();
            let address = self.address;
            let stream = tokio::task::spawn_blocking(move || {
                require_connected(address, &identity, &stream).map_err(|_| ())?;
                Ok::<_, ()>(stream)
            })
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?
            .map_err(|()| StatusCode::BAD_GATEWAY)?;
            TcpStream::from_std(stream).map_err(|_| StatusCode::BAD_GATEWAY)
        })
        .await
        .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
    }
}

async fn relay(
    request: Request,
    upgrade: Option<headers::Upgrade>,
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
) -> ProxyResult<Response> {
    let (mut sender, connection) = timeout(CONNECT_TIMEOUT, http1::handshake(TokioIo::new(stream)))
        .await
        .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let response = sender.send_request(request);
    let mut response = if upgrade.is_some() {
        timeout(UPGRADE_TIMEOUT, response)
            .await
            .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
    } else {
        response.await
    }
    .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if let Some(client) = headers::response(&mut response, upgrade)? {
        let server = hyper::upgrade::on(&mut response);
        tokio::spawn(bridge(client, server));
    }
    Ok(response.map(Body::new))
}

type ProxyResult<T> = std::result::Result<T, StatusCode>;

fn validate_target(method: &Method, uri: &Uri) -> ProxyResult<()> {
    if method == Method::CONNECT {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    let path = uri.path();
    if uri.scheme().is_some() || uri.authority().is_some() || !path.starts_with('/') {
        return Err(StatusCode::BAD_REQUEST);
    }
    if path != API
        && !path
            .strip_prefix(API)
            .is_some_and(|tail| tail.starts_with('/'))
    {
        return Err(StatusCode::NOT_FOUND);
    }
    // 保留上游原始路径及查询，仅拒绝可改变路径分段的编码和无效文本。
    let bytes = path.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'%' {
            let pair = bytes
                .get(index + 1..index + 3)
                .filter(|pair| pair.iter().all(u8::is_ascii_hexdigit))
                .ok_or(StatusCode::BAD_REQUEST)?;
            if pair.eq_ignore_ascii_case(b"2f") {
                return Err(StatusCode::BAD_REQUEST);
            }
        }
    }
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if decoded.chars().any(|ch| ch == '\\' || ch.is_control())
        || decoded[1..]
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(())
}

#[cfg(windows)]
fn require_listener(address: SocketAddrV4, identity: &Value) -> Result<()> {
    if process::loopback_listener(identity, address.port())? {
        Ok(())
    } else {
        Err("登记 API 进程未监听回环端口".into())
    }
}

#[cfg(not(windows))]
fn require_listener(address: SocketAddrV4, identity: &Value) -> Result<()> {
    process::listener(identity, &format!("http://{address}"))
}

fn require_connected(
    address: SocketAddrV4,
    identity: &Value,
    stream: &std::net::TcpStream,
) -> Result<()> {
    if stream.peer_addr()? != std::net::SocketAddr::V4(address) {
        return Err("恢复 API 连接与登记端点不同".into());
    }
    #[cfg(windows)]
    {
        process::require_loopback_peer(identity, stream)
    }
    #[cfg(not(windows))]
    {
        require_listener(address, identity)
    }
}

async fn bridge(client: OnUpgrade, server: OnUpgrade) {
    if let Ok(Ok((client, server))) =
        timeout(UPGRADE_TIMEOUT, async { tokio::try_join!(client, server) }).await
    {
        let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(client), &mut TokioIo::new(server))
            .await;
    }
}
