use reqwest::{Response, Url};
use sha2::{Digest, Sha256};

use super::super::{StorageError, StorageResult};

pub(super) fn normalize_endpoint(endpoint: &str, use_ssl: bool) -> StorageResult<Url> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(StorageError::Configuration(
            "S3 endpoint is required".to_owned(),
        ));
    }
    let scheme = if use_ssl { "https" } else { "http" };
    let raw = if endpoint.contains("://") {
        endpoint.to_owned()
    } else {
        format!("{scheme}://{endpoint}")
    };
    let mut url = Url::parse(&raw)
        .map_err(|error| StorageError::Configuration(format!("invalid S3 endpoint: {error}")))?;
    url.set_scheme(scheme).map_err(|_| {
        StorageError::Configuration("S3 endpoint scheme must be HTTP or HTTPS".to_owned())
    })?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(StorageError::Configuration(
            "S3 endpoint must contain only scheme, host, and optional port".to_owned(),
        ));
    }
    url.set_path("");
    Ok(url)
}

pub(super) fn empty_payload_hash() -> String {
    hex::encode(Sha256::digest([]))
}

pub fn normalize_sha256(value: &str) -> StorageResult<String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StorageError::Signing(
            "file SHA-256 must contain exactly 64 hexadecimal characters".to_owned(),
        ));
    }
    Ok(value.to_ascii_lowercase())
}

pub(super) fn s3_request_result_label(result: &Result<Response, reqwest::Error>) -> &'static str {
    match result {
        Ok(response) if response.status().is_success() => "success",
        Ok(response) if response.status().is_client_error() => "client_error",
        Ok(response) if response.status().is_server_error() => "server_error",
        Ok(_) => "other_http",
        Err(_) => "transport_error",
    }
}

pub(super) fn transport_error(error: reqwest::Error) -> StorageError {
    StorageError::Transport(error.without_url())
}

pub(super) fn service_error(operation: &'static str, response: Response) -> StorageError {
    let status = response.status().as_u16();
    StorageError::Service {
        operation,
        status,
        message: "remote S3 service returned a non-success response".to_owned(),
    }
}
