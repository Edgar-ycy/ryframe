use std::net::Ipv4Addr;

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode, Version, header},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use hyper::upgrade::OnUpgrade;
use sha1::{Digest, Sha1};

use super::{ProxyResult, WEBSOCKET};

const FORWARDED_FOR: &str = "x-forwarded-for";
const WS_KEY: &str = "sec-websocket-key";
const WS_ACCEPT: &str = "sec-websocket-accept";
const WS_VERSION: &str = "sec-websocket-version";

pub(super) struct Upgrade {
    client: OnUpgrade,
    accept: String,
}

pub(super) fn request(request: &mut Request) -> ProxyResult<Option<Upgrade>> {
    let connection = connection_fields(request.headers()).map_err(|()| StatusCode::BAD_REQUEST)?;
    let is_ws = request.uri().path() == WEBSOCKET;
    let has_upgrade =
        request.headers().contains_key(header::UPGRADE) || connection.contains(&header::UPGRADE);
    if has_upgrade && !is_ws {
        return Err(StatusCode::BAD_REQUEST);
    }
    let forwarded_for = single(request.headers(), FORWARDED_FOR)
        .filter(|value| test_address(value))
        .cloned();
    // 不把受管浏览器控制标记或非规范来源头交给产品。
    if is_ws && !empty_upgrade_body(request.headers()) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let headers = request.headers_mut();
    strip(headers, &connection, is_ws);
    for name in [
        FORWARDED_FOR,
        "forwarded",
        "x-forwarded-host",
        "x-forwarded-port",
        "x-forwarded-proto",
        "x-real-ip",
        "x-ryframe-test-gate",
    ] {
        headers.remove(name);
    }
    if let Some(address) = forwarded_for
        && !connection.iter().any(|name| name.as_str() == FORWARDED_FOR)
    {
        headers.insert(FORWARDED_FOR, address);
    }

    if !is_ws {
        return Ok(None);
    }
    let accept = request_accept(request, &connection)?;
    if request.extensions().get::<OnUpgrade>().is_none() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let client = hyper::upgrade::on(&mut *request);
    restore_upgrade(request.headers_mut());
    Ok(Some(Upgrade { client, accept }))
}

pub(super) fn response<B>(
    response: &mut Response<B>,
    upgrade: Option<Upgrade>,
) -> ProxyResult<Option<OnUpgrade>> {
    let connection = connection_fields(response.headers()).map_err(|()| StatusCode::BAD_GATEWAY)?;
    let switched = response.status() == StatusCode::SWITCHING_PROTOCOLS;
    if switched && !empty_upgrade_body(response.headers()) {
        return Err(StatusCode::BAD_GATEWAY);
    }
    let headers = response.headers_mut();
    strip(headers, &connection, switched);
    if !switched {
        return Ok(None);
    }
    let upgrade = upgrade.ok_or(StatusCode::BAD_GATEWAY)?;
    if !websocket(headers, &connection)
        || !single(headers, WS_ACCEPT)
            .is_some_and(|value| value.as_bytes() == upgrade.accept.as_bytes())
    {
        return Err(StatusCode::BAD_GATEWAY);
    }
    restore_upgrade(headers);
    if response.extensions().get::<OnUpgrade>().is_none() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    Ok(Some(upgrade.client))
}

fn request_accept(request: &Request, connection: &[HeaderName]) -> ProxyResult<String> {
    let headers = request.headers();
    if request.method() != Method::GET
        || request.version() != Version::HTTP_11
        || !websocket(headers, connection)
        || single(headers, WS_VERSION).is_none_or(|version| version != "13")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let key = single(headers, WS_KEY).ok_or(StatusCode::BAD_REQUEST)?;
    let decoded = STANDARD
        .decode(key.as_bytes())
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    if decoded.len() != 16 || STANDARD.encode(decoded).as_bytes() != key.as_bytes() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut hash = Sha1::new();
    hash.update(key.as_bytes());
    hash.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    Ok(STANDARD.encode(hash.finalize()))
}

fn websocket(headers: &HeaderMap, connection: &[HeaderName]) -> bool {
    connection.contains(&header::UPGRADE)
        && single(headers, "upgrade")
            .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"))
}

fn empty_upgrade_body(headers: &HeaderMap) -> bool {
    !headers.contains_key(header::TRANSFER_ENCODING)
        && (!headers.contains_key(header::CONTENT_LENGTH)
            || single(headers, "content-length").is_some_and(|length| length == "0"))
}

fn restore_upgrade(headers: &mut HeaderMap) {
    headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
}

fn connection_fields(headers: &HeaderMap) -> std::result::Result<Vec<HeaderName>, ()> {
    let mut fields = Vec::new();
    for value in headers.get_all(header::CONNECTION) {
        for name in value.to_str().map_err(|_| ())?.split(',').map(str::trim) {
            if !name.is_empty() {
                fields.push(HeaderName::from_bytes(name.as_bytes()).map_err(|_| ())?);
            }
        }
    }
    Ok(fields)
}

fn strip(headers: &mut HeaderMap, connection: &[HeaderName], upgrade: bool) {
    for name in connection {
        if !upgrade || name.as_str() != "upgrade" {
            headers.remove(name);
        }
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        headers.remove(name);
    }
    if !upgrade {
        headers.remove(header::UPGRADE);
    }
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a HeaderValue> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn test_address(value: &HeaderValue) -> bool {
    let Ok(text) = value.to_str() else {
        return false;
    };
    text.parse::<Ipv4Addr>().is_ok_and(|ip| {
        let [first, second, ..] = ip.octets();
        first == 198 && matches!(second, 18 | 19) && ip.to_string() == text
    })
}
