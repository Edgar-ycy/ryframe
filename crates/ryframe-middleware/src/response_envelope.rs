use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::{HeaderValue, StatusCode, header, response::Parts},
    middleware::Next,
    response::Response,
};
use ryframe_http::{API_PREFIX, api_path};
use serde_json::{Value, json};

use crate::request_id::RequestId;

/// 统一 JSON 响应在中间件中允许缓冲的最大字节数。
const API_JSON_RESPONSE_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// 统一所有 `/api` 路径的响应信封，并把请求 ID 同步到响应头和响应体。
///
/// 业务处理器只负责构造业务数据；该中间件在压缩前处理响应，避免响应头与
/// 响应体中的请求 ID 分离。文件流和 OpenAPI 文档不受影响。
pub async fn api_response_envelope_middleware(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let is_api_request = is_api_namespace_path(path);
    let bypass_contract_document = is_contract_document_path(path);
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.0.clone())
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let response = next.run(request).await;

    if !is_api_request {
        return response;
    }

    normalize_response(response, &request_id, bypass_contract_document).await
}

fn is_api_namespace_path(path: &str) -> bool {
    let api_root = API_PREFIX
        .rsplit_once('/')
        .map_or(API_PREFIX, |(root, _version)| root);
    path.strip_prefix(api_root)
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
}

fn is_contract_document_path(path: &str) -> bool {
    path == api_path("api-docs/openapi.json")
        || path
            .strip_prefix(&api_path("swagger-ui"))
            .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with('/'))
}

fn is_json_content_type(response: &Response) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .is_some_and(|media_type| {
            media_type.eq_ignore_ascii_case("application/json")
                || media_type.to_ascii_lowercase().ends_with("+json")
        })
}

async fn normalize_response(
    response: Response,
    request_id: &str,
    bypass_contract_document: bool,
) -> Response {
    let status = response.status();

    // 升级协议、OpenAPI/Swagger 文档及成功的非 JSON 响应必须保持流式传输，
    // 不得在统一响应中间件中读取响应体。
    if status == StatusCode::SWITCHING_PROTOCOLS
        || (status.is_success() && bypass_contract_document)
        || (status.is_success() && !is_json_content_type(&response))
    {
        return response;
    }

    // 非 JSON 错误不包含可复用的统一信封，直接丢弃原响应体并生成稳定错误结构。
    if !is_json_content_type(&response) {
        return error_response_from_response(response, status, request_id);
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, API_JSON_RESPONSE_LIMIT_BYTES).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "读取 API 响应体失败");
            return error_response_from_parts(parts, StatusCode::INTERNAL_SERVER_ERROR, request_id);
        }
    };

    let body = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(object)) if contains_removed_response_fields(&object) => {
            tracing::error!("API 响应包含已删除的旧契约字段");
            return error_response_from_parts(parts, StatusCode::INTERNAL_SERVER_ERROR, request_id);
        }
        Ok(Value::Object(mut object)) if is_envelope(&object) => {
            normalize_envelope(&mut object, status, request_id);
            Value::Object(object)
        }
        Ok(_) if status.is_success() => {
            tracing::error!("成功的 JSON API 响应未使用统一响应信封");
            return error_response_from_parts(parts, StatusCode::INTERNAL_SERVER_ERROR, request_id);
        }
        _ => failure_envelope(status, request_id),
    };

    let encoded = match serde_json::to_vec(&body) {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::error!(%error, "序列化统一 API 响应失败");
            return error_response_from_parts(parts, StatusCode::INTERNAL_SERVER_ERROR, request_id);
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
    value.contains_key("code") && value.contains_key("message")
}

fn contains_removed_response_fields(value: &serde_json::Map<String, Value>) -> bool {
    value.contains_key("msg") || value.contains_key("rows") || value.contains_key("total")
}

fn normalize_envelope(
    value: &mut serde_json::Map<String, Value>,
    status: StatusCode,
    request_id: &str,
) {
    let message = value
        .remove("message")
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
    value.clear();
    value.insert("code".into(), json!(status.as_u16()));
    value.insert("message".into(), message);
    value.insert("data".into(), data);
    value.insert("request_id".into(), Value::String(request_id.to_string()));
    value.insert("error_key".into(), error_key);
    value.insert("details".into(), details);
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

fn error_response_from_response(
    response: Response,
    status: StatusCode,
    request_id: &str,
) -> Response {
    let (parts, _body) = response.into_parts();
    error_response_from_parts(parts, status, request_id)
}

fn error_response_from_parts(mut parts: Parts, status: StatusCode, request_id: &str) -> Response {
    let body = failure_envelope(status, request_id).to_string();
    parts.status = status;
    for name in [
        header::CONTENT_LENGTH,
        header::CONTENT_TYPE,
        header::CONTENT_ENCODING,
        header::CONTENT_RANGE,
        header::CONTENT_DISPOSITION,
        header::ETAG,
        header::LAST_MODIFIED,
    ] {
        parts.headers.remove(name);
    }
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Ok(value) = HeaderValue::from_str(request_id) {
        parts.headers.insert("x-request-id", value);
    }
    Response::from_parts(parts, Body::from(body))
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
        http::{HeaderValue, Request, StatusCode, header},
        middleware::from_fn,
        response::Response,
        routing::get,
    };
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use super::api_response_envelope_middleware;
    use crate::request_id::request_id_middleware;
    use ryframe_http::ApiResponse;

    #[tokio::test]
    async fn request_id_matches_between_response_header_and_body() {
        let app = Router::new()
            .route(
                "/api/v1/example",
                get(|| async { Json(ApiResponse::success(json!({ "accepted": true }))) }),
            )
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                Request::builder()
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
        assert_eq!(value.as_object().map(serde_json::Map::len), Some(6));
    }

    #[tokio::test]
    async fn unknown_version_and_unversioned_api_paths_return_enveloped_json_404() {
        let app = Router::new()
            .route("/api/v1/example", get(|| async { "ok" }))
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        for uri in ["/api/v2/example", "/api/example"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .body(Body::empty())
                        .expect("测试请求应可构造"),
                )
                .await
                .expect("路由应返回响应");
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
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

            assert_eq!(value["code"], 404);
            assert_eq!(value["message"], "资源不存在");
            assert_eq!(value["data"], Value::Null);
            assert_eq!(value["request_id"], header_request_id);
            assert_eq!(value["error_key"], "not_found");
            assert_eq!(value["details"], Value::Null);
        }
    }

    #[tokio::test]
    async fn removed_legacy_response_fields_are_rejected() {
        let app = Router::new()
            .route(
                "/api/v1/legacy",
                get(|| async { Json(json!({ "code": 200, "msg": "旧响应", "data": null })) }),
            )
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/legacy")
                    .body(Body::empty())
                    .expect("测试请求应可构造"),
            )
            .await
            .expect("路由应返回响应");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("响应体应可读取");
        let value: Value = serde_json::from_slice(&body).expect("响应体应为 JSON");
        assert_eq!(value["message"], "服务器内部错误");
        assert!(value.get("msg").is_none());
    }

    #[tokio::test]
    async fn successful_unenveloped_json_fails_closed() {
        let app = Router::new()
            .route(
                "/api/v1/raw-json",
                get(|| async { Json(json!({ "accepted": true })) }),
            )
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/raw-json")
                    .body(Body::empty())
                    .expect("测试请求应可构造"),
            )
            .await
            .expect("路由应返回响应");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("响应体应可读取");
        let value: Value = serde_json::from_slice(&body).expect("响应体应为 JSON");
        assert_eq!(value["code"], 500);
        assert_eq!(value["error_key"], "internal");
    }

    #[tokio::test]
    async fn binary_response_preserves_original_body_and_content_type() {
        let app = Router::new()
            .route(
                "/api/v1/download",
                get(|| async {
                    let mut response = Response::new(Body::from(vec![0_u8, 1, 2, 3]));
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("application/octet-stream"),
                    );
                    response
                }),
            )
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/download")
                    .body(Body::empty())
                    .expect("测试请求应可构造"),
            )
            .await
            .expect("路由应返回响应");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/octet-stream"
        );
        let body = to_bytes(response.into_body(), 1024)
            .await
            .expect("二进制响应应可读取");
        assert_eq!(body.as_ref(), &[0, 1, 2, 3]);
    }

    #[tokio::test]
    async fn openapi_json_document_bypasses_business_envelope() {
        let app = Router::new()
            .route(
                "/api/v1/api-docs/openapi.json",
                get(|| async { Json(json!({ "openapi": "3.1.0" })) }),
            )
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/api-docs/openapi.json")
                    .body(Body::empty())
                    .expect("测试请求应可构造"),
            )
            .await
            .expect("路由应返回响应");

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("OpenAPI 响应应可读取");
        let value: Value = serde_json::from_slice(&body).expect("响应体应为 JSON");
        assert_eq!(value["openapi"], "3.1.0");
        assert!(value.get("code").is_none());
    }

    #[tokio::test]
    async fn switching_protocols_response_bypasses_business_envelope() {
        let app = Router::new()
            .route(
                "/api/v1/ws",
                get(|| async {
                    let mut response = Response::new(Body::empty());
                    *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                    response
                }),
            )
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/ws")
                    .body(Body::empty())
                    .expect("测试请求应可构造"),
            )
            .await
            .expect("路由应返回响应");

        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
    }

    #[tokio::test]
    async fn disabled_documentation_openapi_path_returns_enveloped_404() {
        let app = Router::<()>::new()
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/api-docs/openapi.json")
                    .body(Body::empty())
                    .expect("测试请求应可构造"),
            )
            .await
            .expect("路由应返回响应");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("响应体应可读取");
        let value: Value = serde_json::from_slice(&body).expect("响应体应为 JSON");
        assert_eq!(value["code"], 404);
        assert_eq!(value["error_key"], "not_found");
    }

    #[tokio::test]
    async fn error_normalization_preserves_non_content_headers_and_extensions() {
        #[derive(Clone, Debug, PartialEq, Eq)]
        struct ResponseMarker(&'static str);

        let app = Router::new()
            .route(
                "/api/v1/limited",
                get(|| async {
                    let mut response = Response::new(Body::from("too many requests"));
                    *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
                    response.headers_mut().insert(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("text/plain; charset=utf-8"),
                    );
                    response
                        .headers_mut()
                        .insert(header::RETRY_AFTER, HeaderValue::from_static("17"));
                    response.headers_mut().insert(
                        header::ACCESS_CONTROL_ALLOW_ORIGIN,
                        HeaderValue::from_static("https://admin.example.com"),
                    );
                    response.extensions_mut().insert(ResponseMarker("kept"));
                    response
                }),
            )
            .layer(from_fn(api_response_envelope_middleware))
            .layer(from_fn(request_id_middleware));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/limited")
                    .body(Body::empty())
                    .expect("测试请求应可构造"),
            )
            .await
            .expect("路由应返回响应");

        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "17");
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "https://admin.example.com"
        );
        assert_eq!(
            response.extensions().get::<ResponseMarker>(),
            Some(&ResponseMarker("kept"))
        );
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }
}
