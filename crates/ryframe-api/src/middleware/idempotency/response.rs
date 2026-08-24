use axum::{
    body::Body,
    extract::{OriginalUri, Request},
    http::{
        HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
        header::{self, RETRY_AFTER},
    },
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};

use super::{CachedHeader, CachedResponse};
use crate::metrics::record_redis_degraded;

pub(super) fn is_mutating(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// 为幂等命名空间构建具体且规范化的请求目标。
///
/// 有意不使用路由模板（如 `/resources/{id}`）：同一客户端键绝不能将某个具体资源的变更
/// 重放到另一资源。查询参数对会排序，因为其顺序不改变请求语义；重复参数对仍会保留。
pub(super) fn normalized_request_target(request: &Request) -> String {
    let uri = request
        .extensions()
        .get::<OriginalUri>()
        .map_or(request.uri(), |original| &original.0);
    let path = normalize_request_path(uri.path());
    match uri.query() {
        Some(query) if !query.is_empty() => format!("{path}?{}", normalize_query(query)),
        _ => path,
    }
}

/// 在规范化百分号转义大小写的同时保留具体路径的路由语义。尤其是，此处有意不合并
/// 斜杠、不移除尾随斜杠，也不解码保留字符。
fn normalize_request_path(path: &str) -> String {
    if path.is_empty() {
        return "/".to_string();
    }
    normalize_percent_escape_casing(path)
}

/// 在不解码值的情况下规范化查询参数对顺序。这样可保持 `+`、百分号编码的保留字符、
/// 重复键和空值的语义不变，同时让等价的参数顺序共享同一重放记录。
fn normalize_query(query: &str) -> String {
    let mut pairs = query
        .split('&')
        .map(normalize_percent_escape_casing)
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    pairs.join("&")
}

fn normalize_percent_escape_casing(value: &str) -> String {
    let mut normalized = value.as_bytes().to_vec();
    let mut index = 0;
    while index < normalized.len() {
        if normalized[index] == b'%'
            && index + 2 < normalized.len()
            && normalized[index + 1].is_ascii_hexdigit()
            && normalized[index + 2].is_ascii_hexdigit()
        {
            normalized[index + 1] = normalized[index + 1].to_ascii_uppercase();
            normalized[index + 2] = normalized[index + 2].to_ascii_uppercase();
            index += 3;
        } else {
            index += 1;
        }
    }
    String::from_utf8(normalized).unwrap_or_else(|_| value.to_owned())
}

fn is_replayable_response_header(name: &HeaderName) -> bool {
    matches!(name.as_str(), "content-type" | "location" | "etag")
}

pub(super) fn cacheable_response_headers(headers: &HeaderMap) -> Vec<CachedHeader> {
    let mut cached = Vec::new();
    for name in [header::CONTENT_TYPE, header::LOCATION, header::ETAG] {
        for value in headers.get_all(&name) {
            cached.push(CachedHeader {
                name: name.as_str().to_owned(),
                value: value.as_bytes().to_vec(),
            });
        }
    }
    cached
}

/// 存储键只隔离租户、用户与客户端提供的原始幂等键。
/// 请求语义由完整指纹判断回放或冲突。
pub(super) fn storage_key(tenant_id: &str, user_id: i64, raw_key: &str) -> String {
    hex_sha256(format!("{tenant_id}\n{user_id}\n{raw_key}").as_bytes())
}

/// 将租户、用户、方法、具体规范路径、排序后的查询参数与正文 SHA-256 绑定为请求指纹。
pub(super) fn request_fingerprint(
    tenant_id: &str,
    user_id: i64,
    method: &Method,
    request_target: &str,
    body: &[u8],
) -> String {
    let body_sha256 = hex_sha256(body);
    hex_sha256(
        format!(
            "{tenant_id}\n{user_id}\n{}\n{request_target}\n{body_sha256}",
            method.as_str()
        )
        .as_bytes(),
    )
}

fn hex_sha256(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn rebuild_response(cached: CachedResponse) -> Response {
    let status = StatusCode::from_u16(cached.status).unwrap_or(StatusCode::OK);
    let mut response = Response::new(Body::from(cached.body));
    *response.status_mut() = status;
    for cached_header in cached.headers {
        let Ok(name) = HeaderName::from_bytes(cached_header.name.as_bytes()) else {
            continue;
        };
        if !is_replayable_response_header(&name) {
            continue;
        }
        if let Ok(value) = HeaderValue::from_bytes(&cached_header.value) {
            response.headers_mut().append(name, value);
        }
    }
    response
        .headers_mut()
        .insert("X-Idempotency-Replay", HeaderValue::from_static("true"));
    response
}

pub(super) fn conflict_response(message: &str, retry_after_secs: u64) -> Response {
    let mut response = (StatusCode::CONFLICT, message.to_string()).into_response();
    response.headers_mut().insert(
        RETRY_AFTER,
        HeaderValue::from_str(&retry_after_secs.to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("1")),
    );
    response
}

pub(super) fn unavailable_response(error: String) -> Response {
    record_redis_degraded("idempotency");
    tracing::error!(error = %error, "idempotency backend unavailable");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "idempotency service unavailable",
    )
        .into_response()
}
