use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue},
    response::Response,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use ryframe_http::{HttpAppError, HttpResult};
use ryframe_kernel::AppError;

const XLSX_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// 构建采用 RFC 5987 编码的 UTF-8 附件响应头。
pub(crate) fn attachment_content_disposition(filename: &str) -> HttpResult<HeaderValue> {
    let encoded = utf8_percent_encode(filename, NON_ALPHANUMERIC);
    HeaderValue::from_str(&format!("attachment; filename*=UTF-8''{encoded}")).map_err(|error| {
        HttpAppError::from(AppError::Validation(format!(
            "invalid download filename: {error}"
        )))
    })
}

pub(crate) fn tenant_id_from_headers(headers: &HeaderMap) -> HttpResult<String> {
    let tenant_id = headers
        .get("X-Tenant-Id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| AppError::Validation("缺少租户信息".into()))?;
    ryframe_core::validate_tenant_identifier(&tenant_id)?;
    Ok(tenant_id)
}

pub(crate) fn excel_response(bytes: Vec<u8>, filename: &str) -> HttpResult<Response> {
    Response::builder()
        .status(200)
        .header("Content-Type", XLSX_CONTENT_TYPE)
        .header(
            "Content-Disposition",
            attachment_content_disposition(filename)?,
        )
        .body(Body::from(bytes))
        .map_err(|e| HttpAppError::from(AppError::Internal(format!("build response failed: {e}"))))
}

fn parse_i64(value: &str) -> HttpResult<i64> {
    let value = value.trim();
    value
        .parse()
        .map_err(|_| HttpAppError::from(AppError::Validation(format!("无效的ID: {value}"))))
}

pub(crate) fn parse_csv_i64(ids: &str) -> HttpResult<Vec<i64>> {
    ids.split(',').map(parse_i64).collect()
}

pub(crate) fn parse_i64_strings(ids: &[String]) -> HttpResult<Vec<i64>> {
    ids.iter().map(|id| parse_i64(id)).collect()
}

pub(crate) fn parse_optional_i64(id: Option<String>) -> HttpResult<Option<i64>> {
    parse_optional_i64_str(id.as_deref())
}

pub(crate) fn parse_optional_i64_str(id: Option<&str>) -> HttpResult<Option<i64>> {
    id.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(parse_i64)
        .transpose()
}
