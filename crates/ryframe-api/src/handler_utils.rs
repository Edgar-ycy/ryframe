use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue},
    response::Response,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use ryframe_http::{HttpAppError, HttpResult};
use ryframe_kernel::AppError;
use sha2::{Digest, Sha256};

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

pub(crate) fn tenant_id_from_headers(
    headers: &HeaderMap,
    config: &ryframe_config::MultiTenancyConfig,
) -> HttpResult<String> {
    if let Some(tenant_id) = config.fixed_tenant_id() {
        if let Some(requested) = headers.get("X-Tenant-Id") {
            let requested = requested
                .to_str()
                .map(str::trim)
                .map_err(|_| AppError::Validation("租户请求头不是有效文本".into()))?;
            if requested != tenant_id {
                return Err(
                    AppError::Validation(format!("单租户模式只允许使用 {tenant_id} 租户")).into(),
                );
            }
        }
        return Ok(tenant_id.to_owned());
    }
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

/// 构建受控内容类型的通用附件响应。
pub(crate) fn attachment_response(
    bytes: Vec<u8>,
    filename: &str,
    content_type: &str,
) -> HttpResult<Response> {
    Response::builder()
        .status(200)
        .header("Content-Type", content_type)
        .header(
            "Content-Disposition",
            attachment_content_disposition(filename)?,
        )
        .body(Body::from(bytes))
        .map_err(|error| {
            HttpAppError::from(AppError::Internal(format!("构建下载响应失败: {error}")))
        })
}

/// 读取并散列写请求幂等键；数据库和后台任务只保存摘要。
pub(crate) fn idempotency_key_hash(headers: &HeaderMap) -> HttpResult<String> {
    Ok(hex::encode(Sha256::digest(
        idempotency_key_value(headers)?.as_bytes(),
    )))
}

/// 读取数据库幂等服务需要自行散列的原始幂等键；调用方不得记录该值。
pub(crate) fn idempotency_key_value(headers: &HeaderMap) -> HttpResult<String> {
    Ok(headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        })
        .ok_or_else(|| AppError::Validation("缺少有效的 Idempotency-Key 请求头".into()))
        .map(str::to_owned)?)
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
