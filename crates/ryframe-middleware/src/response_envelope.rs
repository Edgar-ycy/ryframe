use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
};
use serde_json::{Value, json};

use crate::request_id::RequestId;

/// 统一 `/api/v1` 响应信封，并把请求 ID 同步到响应头和响应体。
///
/// 业务处理器只负责构造业务数据；该中间件在压缩前处理响应，避免响应头与
/// 响应体中的请求 ID 分离。非 API 路径、文件流和 OpenAPI 文档不受影响。
pub async fn api_response_envelope_middleware(request: Request, next: Next) -> Response {
    let is_api_request = request.uri().path().starts_with("/api/v1");
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.0.clone())
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let response = next.run(request).await;

    if !is_api_request {
        return response;
    }

    normalize_response(response, &request_id).await
}

async fn normalize_response(response: Response, request_id: &str) -> Response {
    let (mut parts, body) = response.into_parts();
    let status = parts.status;
    let bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "读取 API 响应体失败");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, request_id);
        }
    };

    let body = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(mut object)) if is_envelope(&object) => {
            normalize_envelope(&mut object, status, request_id);
            Value::Object(object)
        }
        Ok(Value::Object(_)) if status.is_success() => {
            return Response::from_parts(parts, Body::from(bytes));
        }
        _ if status.is_success() => return Response::from_parts(parts, Body::from(bytes)),
        _ => failure_envelope(status, request_id),
    };

    let encoded = match serde_json::to_vec(&body) {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::error!(%error, "序列化统一 API 响应失败");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, request_id);
        }
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Ok(value) = HeaderValue::from_str(request_id) {
        parts.headers.insert("x-request-id", value);
    }
    Response::from_parts(parts, Body::from(encoded))
}

fn is_envelope(value: &serde_json::Map<String, Value>) -> bool {
    value.contains_key("code") && (value.contains_key("message") || value.contains_key("msg"))
}

fn normalize_envelope(
    value: &mut serde_json::Map<String, Value>,
    status: StatusCode,
    request_id: &str,
) {
    let message = value
        .remove("message")
        .or_else(|| value.remove("msg"))
        .filter(Value::is_string)
        .unwrap_or_else(|| Value::String(default_message(status).to_string()));
    let data = value.remove("data").unwrap_or(Value::Null);
    let error_key = value.remove("error_key").unwrap_or_else(|| {
        if status.is_success() {
            Value::Null
        } else {
            Value::String(error_key(status).to_string())
        }
    });
    let details = value.remove("details").unwrap_or(Value::Null);
    value.insert("code".into(), json!(status.as_u16()));
    value.insert("message".into(), message);
    value.insert("data".into(), data);
    value.insert("request_id".into(), Value::String(request_id.to_string()));
    value.insert("error_key".into(), error_key);
    value.insert("details".into(), details);
    value.remove("rows");
    value.remove("total");
}

fn failure_envelope(status: StatusCode, request_id: &str) -> Value {
    json!({
        "code": status.as_u16(),
        "message": default_message(status),
        "data": null,
        "request_id": request_id,
        "error_key": error_key(status),
        "details": null,
    })
}

fn error_response(status: StatusCode, request_id: &str) -> Response {
    let body = failure_envelope(status, request_id).to_string();
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

fn error_key(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST
        | StatusCode::UNPROCESSABLE_ENTITY
        | StatusCode::UNSUPPORTED_MEDIA_TYPE => "validation",
        StatusCode::UNAUTHORIZED => "authentication",
        StatusCode::FORBIDDEN => "authorization",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::CONFLICT => "conflict",
        StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        StatusCode::SERVICE_UNAVAILABLE => "service_unavailable",
        _ => "internal",
    }
}

fn default_message(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST
        | StatusCode::UNPROCESSABLE_ENTITY
        | StatusCode::UNSUPPORTED_MEDIA_TYPE => "请求参数无效",
        StatusCode::UNAUTHORIZED => "认证失败",
        StatusCode::FORBIDDEN => "没有访问权限",
        StatusCode::NOT_FOUND => "资源不存在",
        StatusCode::CONFLICT => "数据冲突",
        StatusCode::PAYLOAD_TOO_LARGE => "请求体过大",
        StatusCode::TOO_MANY_REQUESTS => "请求过于频繁",
        StatusCode::SERVICE_UNAVAILABLE => "服务暂不可用",
        _ => "服务器内部错误",
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        middleware::from_fn,
        routing::get,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::api_response_envelope_middleware;
    use crate::request_id::request_id_middleware;
    use ryframe_http::ApiResponse;

    #[tokio::test]
    async fn 请求_id_在响应头和响应体中保持一致() {
        let app = Router::new()
            .route(
                "/api/v1/example",
                get(|| async { Json(ApiResponse::success(json!({ "accepted": true }))) }),
            )
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/example")
                    .body(Body::empty())
                    .expect("测试请求应可构造"),
            )
            .await
            .expect("路由应返回响应");
        let header_request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .expect("响应头应携带请求 ID")
            .to_string();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("响应体应可读取");
        let value: Value = serde_json::from_slice(&body).expect("响应体应为 JSON");

        assert_eq!(value["code"], 200);
        assert_eq!(value["request_id"], header_request_id);
        assert_eq!(value["message"], "操作成功");
        assert!(value.get("msg").is_none());
    }
}
