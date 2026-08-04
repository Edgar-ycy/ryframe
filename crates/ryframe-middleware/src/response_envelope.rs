use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, response::Parts},
    middleware::Next,
    response::Response,
};
use ryframe_http::{API_PREFIX, api_path};
use ryframe_http::{QUERY_SUCCESS_MESSAGE_KEY, SUCCESS_MESSAGE_KEY};
use ryframe_i18n::{Locale, Localizer, negotiate_locale};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::request_id::RequestId;

/// 统一 JSON 响应在中间件中允许缓冲的最大字节数。
const API_JSON_RESPONSE_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// 统一所有 `/api` 路径的响应信封，并把请求 ID 同步到响应头和响应体。
///
/// 业务处理器只负责构造业务数据；该中间件在压缩前处理响应，避免响应头与
/// 响应体中的请求 ID 分离。文件流和 OpenAPI 文档不受影响。
pub async fn api_response_envelope_middleware(
    State(localizer): State<Arc<Localizer>>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let is_api_request = is_api_namespace_path(path);
    let bypass_contract_document = is_contract_document_path(path);
    let requested_locale = negotiate_locale(
        request
            .headers()
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok()),
        None,
    );
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|value| value.0.clone())
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let mut response = next.run(request).await;

    if !is_api_request {
        return response;
    }

    let locale = response
        .headers()
        .get(header::CONTENT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .and_then(Locale::parse)
        .unwrap_or(requested_locale);
    ensure_locale_headers(response.headers_mut(), locale);

    normalize_response(
        response,
        &request_id,
        bypass_contract_document,
        &localizer,
        locale,
    )
    .await
}

fn ensure_locale_headers(headers: &mut HeaderMap, locale: Locale) {
    headers.insert(
        header::CONTENT_LANGUAGE,
        HeaderValue::from_static(locale.as_str()),
    );
    let already_varies = headers
        .get_all(header::VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|value| value.trim().eq_ignore_ascii_case("accept-language"));
    if !already_varies {
        headers.append(header::VARY, HeaderValue::from_static("Accept-Language"));
    }
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
    localizer: &Localizer,
    locale: Locale,
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
        return error_response_from_response(response, status, request_id, localizer, locale);
    }

    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, API_JSON_RESPONSE_LIMIT_BYTES).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::error!(%error, "读取 API 响应体失败");
            return error_response_from_parts(
                parts,
                StatusCode::INTERNAL_SERVER_ERROR,
                request_id,
                localizer,
                locale,
            );
        }
    };

    let body = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(object)) if contains_removed_response_fields(&object) => {
            tracing::error!("API 响应包含已删除的旧契约字段");
            return error_response_from_parts(
                parts,
                StatusCode::INTERNAL_SERVER_ERROR,
                request_id,
                localizer,
                locale,
            );
        }
        Ok(Value::Object(mut object)) if is_envelope(&object) => {
            normalize_envelope(&mut object, status, request_id, localizer, locale);
            Value::Object(object)
        }
        Ok(_) if status.is_success() => {
            tracing::error!("成功的 JSON API 响应未使用统一响应信封");
            return error_response_from_parts(
                parts,
                StatusCode::INTERNAL_SERVER_ERROR,
                request_id,
                localizer,
                locale,
            );
        }
        _ => failure_envelope(status, request_id, localizer, locale),
    };

    let encoded = match serde_json::to_vec(&body) {
        Ok(encoded) => encoded,
        Err(error) => {
            tracing::error!(%error, "序列化统一 API 响应失败");
            return error_response_from_parts(
                parts,
                StatusCode::INTERNAL_SERVER_ERROR,
                request_id,
                localizer,
                locale,
            );
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
    localizer: &Localizer,
    locale: Locale,
) {
    let source_message_key = value
        .remove("message")
        .and_then(|message| message.as_str().map(str::to_owned));
    let data = if status.is_success() {
        value.remove("data").unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let error_key = normalized_error_key(value.remove("error_key"), status);
    let message_key = if status.is_success() {
        match source_message_key.as_deref() {
            Some(SUCCESS_MESSAGE_KEY) => SUCCESS_MESSAGE_KEY,
            Some(QUERY_SUCCESS_MESSAGE_KEY) => QUERY_SUCCESS_MESSAGE_KEY,
            _ => SUCCESS_MESSAGE_KEY,
        }
    } else {
        localized_error_message_key(error_key.as_deref())
    };
    let message = localizer.translate(locale, message_key);
    let details = if status.is_client_error() {
        value
            .remove("details")
            .filter(Value::is_object)
            .unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    value.clear();
    value.insert("code".into(), json!(status.as_u16()));
    value.insert("message".into(), Value::String(message));
    value.insert("data".into(), data);
    value.insert("request_id".into(), Value::String(request_id.to_string()));
    value.insert(
        "error_key".into(),
        error_key.map_or(Value::Null, Value::String),
    );
    value.insert("details".into(), details);
}

fn normalized_error_key(error_key: Option<Value>, status: StatusCode) -> Option<String> {
    if status.is_success() {
        return None;
    }
    error_key
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|value| !value.trim().is_empty())
        .or_else(|| Some(error_key_for_status(status).to_owned()))
}

fn localized_error_message_key(error_key: Option<&str>) -> &'static str {
    match error_key {
        Some("validation") => "error.validation",
        Some("authentication") => "error.authentication",
        Some("authorization") => "error.authorization",
        Some("not_found") => "error.not_found",
        Some("conflict") => "error.conflict",
        Some("payload_too_large") => "error.payload_too_large",
        Some("rate_limited") => "error.rate_limited",
        Some("database") => "error.database",
        Some("config") => "error.config",
        Some("service_unavailable") => "error.service_unavailable",
        Some("internal") | None | Some(_) => "error.internal",
    }
}

fn failure_envelope(
    status: StatusCode,
    request_id: &str,
    localizer: &Localizer,
    locale: Locale,
) -> Value {
    let error_key = error_key_for_status(status);
    json!({
        "code": status.as_u16(),
        "message": localizer.translate(locale, localized_error_message_key(Some(error_key))),
        "data": null,
        "request_id": request_id,
        "error_key": error_key,
        "details": null,
    })
}

fn error_response_from_response(
    response: Response,
    status: StatusCode,
    request_id: &str,
    localizer: &Localizer,
    locale: Locale,
) -> Response {
    let (parts, _body) = response.into_parts();
    error_response_from_parts(parts, status, request_id, localizer, locale)
}

fn error_response_from_parts(
    mut parts: Parts,
    status: StatusCode,
    request_id: &str,
    localizer: &Localizer,
    locale: Locale,
) -> Response {
    let body = failure_envelope(status, request_id, localizer, locale).to_string();
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

fn error_key_for_status(status: StatusCode) -> &'static str {
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

#[cfg(test)]
mod tests {
    use axum::{
        Json, Router,
        body::{Body, to_bytes},
        http::{HeaderValue, Request, StatusCode, header},
        middleware::{from_fn, from_fn_with_state},
        response::{IntoResponse, Response},
        routing::get,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;
    use tower::ServiceExt;

    use super::api_response_envelope_middleware;
    use crate::request_id::request_id_middleware;
    use ryframe_http::{ApiResponse, HttpAppError};
    use ryframe_i18n::Localizer;
    use ryframe_kernel::AppError;

    fn localizer() -> Arc<Localizer> {
        Arc::new(Localizer::embedded().expect("内嵌国际化资源应有效"))
    }

    #[tokio::test]
    async fn request_id_matches_between_response_header_and_body() {
        let app = Router::new()
            .route(
                "/api/v1/example",
                get(|| async { Json(ApiResponse::success(json!({ "accepted": true }))) }),
            )
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
    async fn supported_locales_render_exact_success_and_error_messages() {
        let app = Router::new()
            .route(
                "/api/v1/success",
                get(|| async {
                    Json(ApiResponse::success(json!({ "accepted": true }))).into_response()
                }),
            )
            .route(
                "/api/v1/validation",
                get(|| async {
                    HttpAppError::from(AppError::Validation("不得出现在响应中的校验细节".into()))
                        .into_response()
                }),
            )
            .route(
                "/api/v1/conflict",
                get(|| async {
                    HttpAppError::from(AppError::Conflict("不得出现在响应中的业务细节".into()))
                        .into_response()
                }),
            )
            .route(
                "/api/v1/unknown",
                get(|| async {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "mysql://user:secret@internal.example/private",
                    )
                        .into_response()
                }),
            )
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
            .layer(from_fn(request_id_middleware));

        for (language, expected) in [
            (
                "zh-CN",
                [
                    ("success", "操作成功", Value::Null),
                    ("validation", "数据校验失败", json!("validation")),
                    ("conflict", "数据冲突", json!("conflict")),
                    ("unknown", "服务器内部错误", json!("internal")),
                ],
            ),
            (
                "en-US",
                [
                    ("success", "Operation successful", Value::Null),
                    ("validation", "Data validation failed", json!("validation")),
                    ("conflict", "Data conflict", json!("conflict")),
                    ("unknown", "Internal server error", json!("internal")),
                ],
            ),
        ] {
            for (path, message, error_key) in expected {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .uri(format!("/api/v1/{path}"))
                            .header(header::ACCEPT_LANGUAGE, language)
                            .body(Body::empty())
                            .expect("本地化测试请求应可构造"),
                    )
                    .await
                    .expect("本地化测试路由应返回响应");
                let request_id = response
                    .headers()
                    .get("x-request-id")
                    .and_then(|value| value.to_str().ok())
                    .expect("本地化响应应携带请求 ID")
                    .to_owned();
                assert_eq!(
                    response.headers().get(header::CONTENT_LANGUAGE),
                    Some(&HeaderValue::from_static(language))
                );
                let body = to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .expect("本地化响应体应可读取");
                let value: Value = serde_json::from_slice(&body).expect("本地化响应体应为 JSON");

                assert_eq!(value["message"], message);
                assert_eq!(value["error_key"], error_key);
                assert_eq!(value["request_id"], request_id);
                assert_eq!(value["details"], Value::Null);
                assert!(!body.windows(6).any(|window| window == b"secret"));
            }
        }
    }

    #[tokio::test]
    async fn response_locale_header_is_the_single_source_for_message_language() {
        let app = Router::new()
            .route(
                "/api/v1/preferred",
                get(|| async {
                    (
                        [(header::CONTENT_LANGUAGE, "en-US")],
                        Json(ApiResponse::success(json!({ "accepted": true }))),
                    )
                }),
            )
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
            .layer(from_fn(request_id_middleware));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/preferred")
                    .header(header::ACCEPT_LANGUAGE, "zh-CN")
                    .body(Body::empty())
                    .expect("用户偏好语言测试请求应可构造"),
            )
            .await
            .expect("用户偏好语言测试路由应返回响应");

        assert_eq!(response.headers()[header::CONTENT_LANGUAGE], "en-US");
        let body = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("用户偏好语言响应体应可读取");
        let value: Value = serde_json::from_slice(&body).expect("响应体应为 JSON");
        assert_eq!(value["message"], "Operation successful");
    }

    #[tokio::test]
    async fn error_keys_and_safe_details_survive_localization() {
        let app = Router::new()
            .route(
                "/api/v1/validation-details",
                get(|| async {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "code": 400,
                            "message": "不得作为最终文案",
                            "data": null,
                            "request_id": "旧请求标识",
                            "error_key": "validation",
                            "details": { "field": "name", "rule": "required" }
                        })),
                    )
                }),
            )
            .route(
                "/api/v1/future-error",
                get(|| async {
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "code": 400,
                            "message": "未知错误键不得绕过安全回退",
                            "data": null,
                            "request_id": "旧请求标识",
                            "error_key": "future_error",
                            "details": null
                        })),
                    )
                }),
            )
            .route(
                "/api/v1/internal-details",
                get(|| async {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "code": 500,
                            "message": "不得作为最终文案",
                            "data": null,
                            "request_id": "旧请求标识",
                            "error_key": "internal",
                            "details": { "dsn": "mysql://user:secret@internal" }
                        })),
                    )
                }),
            )
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
            .layer(from_fn(request_id_middleware));

        let validation = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/validation-details")
                    .header(header::ACCEPT_LANGUAGE, "en-US")
                    .body(Body::empty())
                    .expect("校验详情测试请求应可构造"),
            )
            .await
            .expect("校验详情测试路由应返回响应");
        let validation_body = to_bytes(validation.into_body(), 1024 * 1024)
            .await
            .expect("校验详情响应体应可读取");
        let validation_value: Value =
            serde_json::from_slice(&validation_body).expect("校验详情响应体应为 JSON");
        assert_eq!(validation_value["message"], "Data validation failed");
        assert_eq!(validation_value["error_key"], "validation");
        assert_eq!(
            validation_value["details"],
            json!({ "field": "name", "rule": "required" })
        );

        let future = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/future-error")
                    .header(header::ACCEPT_LANGUAGE, "en-US")
                    .body(Body::empty())
                    .expect("未知错误键测试请求应可构造"),
            )
            .await
            .expect("未知错误键测试路由应返回响应");
        let future_body = to_bytes(future.into_body(), 1024 * 1024)
            .await
            .expect("未知错误键响应体应可读取");
        let future_value: Value =
            serde_json::from_slice(&future_body).expect("未知错误键响应体应为 JSON");
        assert_eq!(future_value["message"], "Internal server error");
        assert_eq!(future_value["error_key"], "future_error");

        let internal = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/internal-details")
                    .header(header::ACCEPT_LANGUAGE, "en-US")
                    .body(Body::empty())
                    .expect("内部错误详情测试请求应可构造"),
            )
            .await
            .expect("内部错误详情测试路由应返回响应");
        let internal_body = to_bytes(internal.into_body(), 1024 * 1024)
            .await
            .expect("内部错误详情响应体应可读取");
        let internal_value: Value =
            serde_json::from_slice(&internal_body).expect("内部错误详情响应体应为 JSON");
        assert_eq!(internal_value["message"], "Internal server error");
        assert_eq!(internal_value["error_key"], "internal");
        assert_eq!(internal_value["details"], Value::Null);
        assert!(!internal_body.windows(6).any(|window| window == b"secret"));
    }

    #[tokio::test]
    async fn unknown_version_and_unversioned_api_paths_return_enveloped_json_404() {
        let app = Router::new()
            .route("/api/v1/example", get(|| async { "ok" }))
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
            .layer(from_fn_with_state(
                localizer(),
                api_response_envelope_middleware,
            ))
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
