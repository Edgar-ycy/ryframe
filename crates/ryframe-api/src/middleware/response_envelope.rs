//! API 统一响应信封。

use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, response::Parts},
    middleware::Next,
    response::Response,
};
use ryframe_http::{API_PREFIX, api_path};
use ryframe_http::{QUERY_SUCCESS_MESSAGE_KEY, SUCCESS_MESSAGE_KEY};
use ryframe_kernel::{Locale, Localizer};
use serde_json::{Value, json};
use std::sync::Arc;

use super::request_id::RequestId;

/// 统一 JSON 响应在中间件中允许缓冲的最大字节数。
const API_JSON_RESPONSE_LIMIT_BYTES: usize = 16 * 1024 * 1024;

/// 标记已经在业务事务内完成最终编码与大小校验的 API 响应。
///
/// Agent 查询需要让审计中的响应字节数与实际发送的未压缩 JSON 完全一致，因此统一响应层只补语言头，
/// 不得再次解析或序列化这类成功响应。错误响应仍走标准安全信封。
#[derive(Clone, Copy, Debug)]
pub struct PrebuiltApiEnvelope;

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
        .unwrap_or(Locale::DEFAULT);
    ensure_locale_headers(response.headers_mut(), locale);

    if response.status().is_success()
        && response.extensions().get::<PrebuiltApiEnvelope>().is_some()
    {
        return response;
    }

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
        Some("capability_unavailable") => "error.capability_unavailable",
        Some("tenant_capability_denied") => "error.tenant_capability_denied",
        Some("permission_denied") => "error.permission_denied",
        Some("stale_runtime_epoch") => "error.stale_runtime_epoch",
        Some("stale_placement_generation") => "error.stale_placement_generation",
        Some("tenant_operation_conflict") => "error.tenant_operation_conflict",
        Some("tenant_data_maintenance") => "error.tenant_data_maintenance",
        Some("tenant_data_target_unavailable") => "error.tenant_data_target_unavailable",
        Some("EXPORT_ALL_CONFIRMATION_REQUIRED") => "error.export_all_confirmation_required",
        Some("EXPORT_NO_MATCHING_ROWS") => "error.export_no_matching_rows",
        Some("EXPORT_ROW_LIMIT_EXCEEDED") => "error.export_row_limit_exceeded",
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
        StatusCode::NOT_IMPLEMENTED => "capability_unavailable",
        _ => "internal",
    }
}
