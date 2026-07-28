use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue},
    response::Response,
};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use ryframe_http::{AppError, AppResult};

const XLSX_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// 构建可安全用于 HTTP 的附件响应头，并为支持 RFC 5987 的客户端保留 UTF-8
/// 文件名。为旧客户端保留 ASCII `filename` 回退值，其中绝不包含分隔符或控制字符。
pub(crate) fn attachment_content_disposition(filename: &str) -> AppResult<HeaderValue> {
    let fallback = ascii_filename_fallback(filename);
    let encoded = utf8_percent_encode(filename, NON_ALPHANUMERIC);
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{fallback}\"; filename*=UTF-8''{encoded}"
    ))
    .map_err(|error| AppError::Validation(format!("invalid download filename: {error}")))
}

fn ascii_filename_fallback(filename: &str) -> String {
    let mut fallback = String::with_capacity(filename.len().min(128));
    for character in filename.chars() {
        match character {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '.' | '-' | '_' => fallback.push(character),
            ' ' => fallback.push('_'),
            _ => {}
        }
        if fallback.len() == 120 {
            break;
        }
    }
    let fallback = fallback.trim_matches(['.', '_']).to_owned();
    if fallback.is_empty() {
        "download".into()
    } else {
        fallback
    }
}

pub(crate) fn tenant_id_from_headers(headers: &HeaderMap) -> AppResult<String> {
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

pub(crate) fn excel_response(bytes: Vec<u8>, filename: &str) -> AppResult<Response> {
    Response::builder()
        .status(200)
        .header("Content-Type", XLSX_CONTENT_TYPE)
        .header(
            "Content-Disposition",
            attachment_content_disposition(filename)?,
        )
        .body(Body::from(bytes))
        .map_err(|e| AppError::Internal(format!("build response failed: {e}")))
}

fn parse_i64(value: &str) -> AppResult<i64> {
    let value = value.trim();
    value
        .parse()
        .map_err(|_| AppError::Validation(format!("无效的ID: {value}")))
}

pub(crate) fn parse_csv_i64(ids: &str) -> AppResult<Vec<i64>> {
    ids.split(',').map(parse_i64).collect()
}

pub(crate) fn parse_i64_strings(ids: &[String]) -> AppResult<Vec<i64>> {
    ids.iter().map(|id| parse_i64(id)).collect()
}

pub(crate) fn parse_optional_i64(id: Option<String>) -> AppResult<Option<i64>> {
    parse_optional_i64_str(id.as_deref())
}

pub(crate) fn parse_optional_i64_str(id: Option<&str>) -> AppResult<Option<i64>> {
    id.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(parse_i64)
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_ids_instead_of_dropping_them() {
        assert!(parse_csv_i64("1,invalid,3").is_err());
        assert!(parse_i64_strings(&["1".into(), "invalid".into()]).is_err());
        assert!(parse_optional_i64_str(Some("invalid")).is_err());
    }

    #[test]
    fn parses_valid_and_empty_optional_ids() {
        assert_eq!(parse_csv_i64("1, 2,3").unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_optional_i64_str(Some("  ")).unwrap(), None);
        assert_eq!(parse_optional_i64_str(Some("42")).unwrap(), Some(42));
    }

    #[test]
    fn extracts_required_tenant_header() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Tenant-Id", " tenant-a ".parse().unwrap());
        assert_eq!(tenant_id_from_headers(&headers).unwrap(), "tenant-a");
        headers.insert("X-Tenant-Id", "**".parse().unwrap());
        assert!(tenant_id_from_headers(&headers).is_err());
        assert!(tenant_id_from_headers(&HeaderMap::new()).is_err());
    }

    #[test]
    fn attachment_header_is_rfc5987_safe_for_unicode_and_controls() {
        let header = attachment_content_disposition("报告\r\n\".xlsx").unwrap();
        let header = header.to_str().unwrap();

        assert!(header.starts_with("attachment; filename=\"xlsx\""));
        assert!(header.contains("filename*=UTF-8''%E6%8A%A5%E5%91%8A%0D%0A%22%2Exlsx"));
        assert!(!header.contains('\r'));
        assert!(!header.contains('\n'));
    }
}
