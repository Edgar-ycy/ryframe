use axum::{
    body::Body,
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// XSS 过滤中间件
///
/// 仅处理 Content-Type=application/json 的请求体
/// 递归遍历 JSON，对所有字符串值应用 ammonia 净化
pub async fn xss_filter(req: Request, next: Next) -> Result<Response, Response> {
    let is_json = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.starts_with("application/json"))
        .unwrap_or(false);

    if !is_json {
        return Ok(next.run(req).await);
    }

    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1024 * 1024 * 2)
        .await
        .map_err(|_| (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response())?;

    let sanitized = sanitize_json_bytes(&bytes);
    let new_req = Request::from_parts(parts, Body::from(sanitized));
    Ok(next.run(new_req).await)
}

pub fn sanitize_json_bytes(input: &axum::body::Bytes) -> axum::body::Bytes {
    let mut v: serde_json::Value = match serde_json::from_slice(input) {
        Ok(v) => v,
        Err(_) => return input.clone(),
    };
    sanitize_value(&mut v);
    axum::body::Bytes::from(serde_json::to_vec(&v).unwrap_or_default())
}

fn sanitize_value(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => *s = ammonia::clean(s),
        serde_json::Value::Array(arr) => arr.iter_mut().for_each(sanitize_value),
        serde_json::Value::Object(map) => map.values_mut().for_each(sanitize_value),
        _ => {}
    }
}
