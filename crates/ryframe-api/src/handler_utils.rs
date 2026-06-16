use axum::{body::Body, response::Response};
use ryframe_common::{AppError, AppResult};

const XLSX_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

pub(crate) fn excel_response(bytes: Vec<u8>, filename: &str) -> AppResult<Response> {
    Response::builder()
        .status(200)
        .header("Content-Type", XLSX_CONTENT_TYPE)
        .header(
            "Content-Disposition",
            format!("attachment; filename={filename}"),
        )
        .body(Body::from(bytes))
        .map_err(|e| AppError::Internal(format!("build response failed: {e}")))
}

pub(crate) fn parse_csv_i64(ids: &str) -> Vec<i64> {
    ids.split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect()
}

pub(crate) fn parse_i64_strings(ids: &[String]) -> Vec<i64> {
    ids.iter().filter_map(|s| s.parse().ok()).collect()
}

pub(crate) fn parse_optional_i64(id: Option<String>) -> Option<i64> {
    parse_optional_i64_str(id.as_deref())
}

pub(crate) fn parse_optional_i64_str(id: Option<&str>) -> Option<i64> {
    id.and_then(|s| s.parse().ok())
}
