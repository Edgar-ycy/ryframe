use std::{io::SeekFrom, path::Path, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{StreamExt, stream};
use quick_xml::{Reader, escape::unescape, events::Event};
use reqwest::{Body, Method, RequestBuilder, Response, Url};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::Instrument;

use super::{
    ObjectListPage, ObjectStorage, StorageError, StorageOperation, StorageResult, key_segments,
    signing::SigV4Signer, storage_operation_span, trace_storage_operation, validate_bucket,
    validate_list_request,
};

const MAX_LIST_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// 路径风格 S3 兼容端点的连接与签名配置。
#[derive(Clone, Debug)]
pub struct S3Config {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub use_ssl: bool,
    pub region: String,
}

/// 适用于 AWS S3 与 MinIO 的 S3 兼容 HTTP 后端。
pub struct S3ObjectStorage {
    endpoint: Url,
    access_key: String,
    secret_key: String,
    region: String,
    client: reqwest::Client,
}

impl S3ObjectStorage {
    pub fn new(config: S3Config) -> StorageResult<Self> {
        if config.access_key.trim().is_empty() || config.secret_key.is_empty() {
            return Err(StorageError::Configuration(
                "S3 access_key and secret_key are required".to_owned(),
            ));
        }
        if config.region.is_empty()
            || !config
                .region
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(StorageError::Configuration(
                "S3 region must contain only letters, digits, or hyphens".to_owned(),
            ));
        }

        let endpoint = normalize_endpoint(&config.endpoint, config.use_ssl)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            endpoint,
            access_key: config.access_key,
            secret_key: config.secret_key,
            region: config.region,
            client,
        })
    }

    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    pub async fn bucket_exists(&self, bucket: &str) -> StorageResult<bool> {
        let url = self.bucket_url(bucket)?;
        let payload_hash = empty_payload_hash();
        let response = self
            .send_request(
                StorageOperation::BucketHead,
                self.signed_request(Method::HEAD, url, &payload_hash)?,
            )
            .await?;
        match response.status().as_u16() {
            200..=299 => Ok(true),
            404 => Ok(false),
            _ => Err(service_error("check S3 bucket", response).await),
        }
    }

    pub async fn create_bucket(&self, bucket: &str) -> StorageResult<()> {
        let url = self.bucket_url(bucket)?;
        let body = if self.region == "us-east-1" {
            String::new()
        } else {
            format!(
                "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>",
                self.region
            )
        };
        let payload_hash = hex::encode(Sha256::digest(body.as_bytes()));
        let mut request = self.signed_request(Method::PUT, url, &payload_hash)?;
        if !body.is_empty() {
            request = request.header("Content-Type", "application/xml").body(body);
        }
        let response = self
            .send_request(StorageOperation::BucketCreate, request)
            .await?;
        if response.status().is_success() {
            return Ok(());
        }
        if response.status().as_u16() == 409 && self.bucket_exists(bucket).await? {
            return Ok(());
        }
        Err(service_error("create S3 bucket", response).await)
    }

    async fn enforce_private_bucket(&self, bucket: &str) -> StorageResult<()> {
        let mut acl_url = self.bucket_url(bucket)?;
        acl_url.set_query(Some("acl"));
        let response = self
            .send_request(
                StorageOperation::BucketSetAcl,
                self.signed_request(Method::PUT, acl_url, empty_payload_hash().as_str())?
                    .header("x-amz-acl", "private"),
            )
            .await?;
        if !response.status().is_success() {
            return Err(service_error("set private S3 bucket ACL", response).await);
        }

        let mut policy_url = self.bucket_url(bucket)?;
        policy_url.set_query(Some("policy"));
        let response = self
            .send_request(
                StorageOperation::BucketGetPolicy,
                self.signed_request(Method::GET, policy_url, "UNSIGNED-PAYLOAD")?,
            )
            .await?;
        if response.status().as_u16() == 404 {
            return Ok(());
        }
        if !response.status().is_success() {
            return Err(service_error("verify S3 bucket policy", response).await);
        }
        let policy = response.text().await?;
        let policy: Value = serde_json::from_str(&policy).map_err(|error| {
            StorageError::Configuration(format!("bucket '{bucket}' policy is invalid: {error}"))
        })?;
        if policy_allows_public_access(&policy) {
            return Err(StorageError::Configuration(format!(
                "bucket '{bucket}' has a public access policy; RyFrame files must remain private"
            )));
        }
        Ok(())
    }

    fn bucket_url(&self, bucket: &str) -> StorageResult<Url> {
        validate_bucket(bucket)?;
        self.location_url(bucket, None)
    }

    fn object_url(&self, bucket: &str, key: &str) -> StorageResult<Url> {
        validate_bucket(bucket)?;
        self.location_url(bucket, Some(key_segments(key)?))
    }

    pub fn list_url(
        &self,
        bucket: &str,
        prefix: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> StorageResult<Url> {
        validate_list_request(bucket, prefix, cursor, limit)?;
        let mut url = self.bucket_url(bucket)?;
        let mut query = url.query_pairs_mut();
        query
            .append_pair("list-type", "2")
            .append_pair("prefix", prefix)
            .append_pair("max-keys", &limit.to_string());
        if let Some(cursor) = cursor {
            query.append_pair("continuation-token", cursor);
        }
        drop(query);
        Ok(url)
    }

    fn location_url(&self, bucket: &str, key: Option<Vec<&str>>) -> StorageResult<Url> {
        let mut url = self.endpoint.clone();
        let mut path = url.path_segments_mut().map_err(|_| {
            StorageError::Configuration("S3 endpoint cannot be a base URL".to_owned())
        })?;
        path.pop_if_empty().push(bucket);
        if let Some(segments) = key {
            path.extend(segments);
        }
        drop(path);
        Ok(url)
    }

    fn signed_request(
        &self,
        method: Method,
        url: Url,
        payload_hash: &str,
    ) -> StorageResult<RequestBuilder> {
        let amz_date = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        let authorization = SigV4Signer {
            access_key: &self.access_key,
            secret_key: &self.secret_key,
            region: &self.region,
        }
        .authorization(method.as_str(), &url, payload_hash, &amz_date)?;

        Ok(self
            .client
            .request(method, url)
            .header("x-amz-content-sha256", payload_hash)
            .header("x-amz-date", amz_date)
            .header("Authorization", authorization))
    }

    async fn send_request(
        &self,
        operation: StorageOperation,
        request: RequestBuilder,
    ) -> StorageResult<Response> {
        let span = storage_operation_span("s3", operation);
        let result = request.send().instrument(span.clone()).await;
        span.record("storage.result", s3_request_result_label(&result));
        result.map_err(StorageError::from)
    }

    pub async fn prepare_upload_file(
        path: &Path,
        supplied_sha256: Option<&str>,
    ) -> StorageResult<(tokio::fs::File, u64, String)> {
        let mut file = tokio::fs::File::open(path)
            .await
            .map_err(|source| StorageError::Io {
                operation: "open S3 upload source",
                source,
            })?;
        let metadata = file.metadata().await.map_err(|source| StorageError::Io {
            operation: "inspect S3 upload source",
            source,
        })?;
        if !metadata.is_file() {
            return Err(StorageError::InvalidLocation(
                "upload source must be a regular file".to_owned(),
            ));
        }

        let payload_hash = match supplied_sha256 {
            Some(value) => normalize_sha256(value)?,
            None => {
                let mut hasher = Sha256::new();
                let mut buffer = vec![0u8; 64 * 1024];
                loop {
                    let read = file
                        .read(&mut buffer)
                        .await
                        .map_err(|source| StorageError::Io {
                            operation: "hash S3 upload source",
                            source,
                        })?;
                    if read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..read]);
                }
                file.seek(SeekFrom::Start(0))
                    .await
                    .map_err(|source| StorageError::Io {
                        operation: "rewind S3 upload source",
                        source,
                    })?;
                hex::encode(hasher.finalize())
            }
        };

        Ok((file, metadata.len(), payload_hash))
    }

    async fn read_bounded_list_response(response: Response) -> StorageResult<Vec<u8>> {
        if response
            .content_length()
            .is_some_and(|length| length > MAX_LIST_RESPONSE_BYTES as u64)
        {
            return Err(StorageError::InvalidResponse(format!(
                "S3 object list response exceeds {MAX_LIST_RESPONSE_BYTES} bytes"
            )));
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let next_length = body.len().checked_add(chunk.len()).ok_or_else(|| {
                StorageError::InvalidResponse("S3 object list response length overflow".to_owned())
            })?;
            if next_length > MAX_LIST_RESPONSE_BYTES {
                return Err(StorageError::InvalidResponse(format!(
                    "S3 object list response exceeds {MAX_LIST_RESPONSE_BYTES} bytes"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

#[async_trait]
impl ObjectStorage for S3ObjectStorage {
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()> {
        let url = self.object_url(bucket, key)?;
        let payload_hash = hex::encode(Sha256::digest(data));
        let response = self
            .send_request(
                StorageOperation::Put,
                self.signed_request(Method::PUT, url, &payload_hash)?
                    .header("Content-Type", content_type)
                    .body(data.to_vec()),
            )
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(service_error("upload S3 object", response).await)
        }
    }

    async fn put_control(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()> {
        self.put(bucket, key, data, content_type).await
    }

    async fn put_file(
        &self,
        bucket: &str,
        key: &str,
        path: &Path,
        content_type: &str,
        sha256_hex: Option<&str>,
    ) -> StorageResult<()> {
        let url = self.object_url(bucket, key)?;
        let (file, content_length, payload_hash) =
            Self::prepare_upload_file(path, sha256_hex).await?;
        let chunks = stream::try_unfold(file, |mut file| async move {
            let mut chunk = vec![0u8; 64 * 1024];
            let read = file.read(&mut chunk).await?;
            if read == 0 {
                return Ok(None);
            }
            chunk.truncate(read);
            Ok::<_, std::io::Error>(Some((chunk, file)))
        });
        let response = self
            .send_request(
                StorageOperation::Put,
                self.signed_request(Method::PUT, url, &payload_hash)?
                    .header("Content-Type", content_type)
                    .header("Content-Length", content_length)
                    .body(Body::wrap_stream(chunks)),
            )
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(service_error("upload S3 object", response).await)
        }
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        let url = self.object_url(bucket, key)?;
        let response = self
            .send_request(
                StorageOperation::Get,
                self.signed_request(Method::GET, url, "UNSIGNED-PAYLOAD")?,
            )
            .await?;
        if !response.status().is_success() {
            return Err(service_error("download S3 object", response).await);
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(StorageError::from)
    }

    async fn get_bounded(
        &self,
        bucket: &str,
        key: &str,
        max_bytes: usize,
    ) -> StorageResult<Vec<u8>> {
        if max_bytes == 0 {
            return Err(StorageError::InvalidLocation(
                "bounded object read limit must be greater than zero".to_owned(),
            ));
        }
        let url = self.object_url(bucket, key)?;
        let mut response = self
            .send_request(
                StorageOperation::Get,
                self.signed_request(Method::GET, url, "UNSIGNED-PAYLOAD")?
                    .header("Range", format!("bytes=0-{max_bytes}")),
            )
            .await?;
        if !response.status().is_success() {
            return Err(service_error("download bounded S3 object", response).await);
        }
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(StorageError::InvalidResponse(
                "object exceeds bounded read limit".to_owned(),
            ));
        }
        let mut data = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or_default()
                .min(max_bytes as u64) as usize,
        );
        while let Some(chunk) = response.chunk().await.map_err(StorageError::from)? {
            if data.len().saturating_add(chunk.len()) > max_bytes {
                return Err(StorageError::InvalidResponse(
                    "object exceeds bounded read limit".to_owned(),
                ));
            }
            data.extend_from_slice(&chunk);
        }
        Ok(data)
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        let url = self.object_url(bucket, key)?;
        let payload_hash = empty_payload_hash();
        let response = self
            .send_request(
                StorageOperation::Delete,
                self.signed_request(Method::DELETE, url, &payload_hash)?,
            )
            .await?;
        if response.status().is_success() || response.status().as_u16() == 404 {
            Ok(())
        } else {
            Err(service_error("delete S3 object", response).await)
        }
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        let url = self.object_url(bucket, key)?;
        let payload_hash = empty_payload_hash();
        let response = self
            .send_request(
                StorageOperation::ObjectHead,
                self.signed_request(Method::HEAD, url, &payload_hash)?,
            )
            .await?;
        match response.status().as_u16() {
            200..=299 => Ok(true),
            404 => Ok(false),
            _ => Err(service_error("check S3 object", response).await),
        }
    }

    async fn list_page(
        &self,
        bucket: &str,
        prefix: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> StorageResult<ObjectListPage> {
        trace_storage_operation("s3", StorageOperation::List, async {
            let url = self.list_url(bucket, prefix, cursor, limit)?;
            let response = self
                .send_request(
                    StorageOperation::List,
                    self.signed_request(Method::GET, url, "UNSIGNED-PAYLOAD")?,
                )
                .await?;
            if !response.status().is_success() {
                return Err(service_error("list S3 objects", response).await);
            }
            let body = Self::read_bounded_list_response(response).await?;
            parse_list_objects_response(&body, prefix, limit)
        })
        .await
    }

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        trace_storage_operation("s3", StorageOperation::EnsureBucket, async {
            if !self.bucket_exists(bucket).await? {
                self.create_bucket(bucket).await?;
            }
            self.enforce_private_bucket(bucket).await
        })
        .await
    }

    async fn readiness_check(&self, bucket: &str) -> StorageResult<()> {
        trace_storage_operation("s3", StorageOperation::Readiness, async {
            if self.bucket_exists(bucket).await? {
                Ok(())
            } else {
                Err(StorageError::Readiness(format!(
                    "required bucket '{bucket}' does not exist"
                )))
            }
        })
        .await
    }
}

pub fn parse_list_objects_response(
    body: &[u8],
    expected_prefix: &str,
    limit: usize,
) -> StorageResult<ObjectListPage> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut keys = Vec::new();
    let mut next_cursor = None;
    let mut is_truncated = None;
    loop {
        match reader.read_event().map_err(|error| {
            StorageError::InvalidResponse(format!("invalid S3 object list XML: {error}"))
        })? {
            Event::Start(element) if element.local_name().as_ref() == b"Key" => {
                let name = element.name();
                let text = reader.read_text(name).map_err(|error| {
                    StorageError::InvalidResponse(format!("invalid S3 object key element: {error}"))
                })?;
                let decoded = text.decode().map_err(|error| {
                    StorageError::InvalidResponse(format!(
                        "invalid S3 object key encoding: {error}"
                    ))
                })?;
                let key = unescape(&decoded)
                    .map_err(|error| {
                        StorageError::InvalidResponse(format!(
                            "invalid S3 object key escaping: {error}"
                        ))
                    })?
                    .into_owned();
                key_segments(&key)?;
                if !key.starts_with(expected_prefix) {
                    return Err(StorageError::InvalidResponse(
                        "S3 object list returned a key outside the requested prefix".to_owned(),
                    ));
                }
                if keys.last().is_some_and(|previous| previous >= &key) {
                    return Err(StorageError::InvalidResponse(
                        "S3 object list keys are not strictly ordered".to_owned(),
                    ));
                }
                keys.push(key);
                if keys.len() > limit {
                    return Err(StorageError::InvalidResponse(
                        "S3 object list returned more keys than requested".to_owned(),
                    ));
                }
            }
            Event::Start(element) if element.local_name().as_ref() == b"NextContinuationToken" => {
                let name = element.name();
                let text = reader.read_text(name).map_err(|error| {
                    StorageError::InvalidResponse(format!(
                        "invalid S3 continuation token element: {error}"
                    ))
                })?;
                let decoded = text.decode().map_err(|error| {
                    StorageError::InvalidResponse(format!(
                        "invalid S3 continuation token encoding: {error}"
                    ))
                })?;
                let token = unescape(&decoded)
                    .map_err(|error| {
                        StorageError::InvalidResponse(format!(
                            "invalid S3 continuation token escaping: {error}"
                        ))
                    })?
                    .into_owned();
                if token.is_empty() || token.len() > 4_096 || token.chars().any(char::is_control) {
                    return Err(StorageError::InvalidResponse(
                        "S3 continuation token is invalid".to_owned(),
                    ));
                }
                next_cursor = Some(token);
            }
            Event::Start(element) if element.local_name().as_ref() == b"IsTruncated" => {
                let name = element.name();
                let text = reader.read_text(name).map_err(|error| {
                    StorageError::InvalidResponse(format!("invalid S3 truncation element: {error}"))
                })?;
                let decoded = text.decode().map_err(|error| {
                    StorageError::InvalidResponse(format!(
                        "invalid S3 truncation flag encoding: {error}"
                    ))
                })?;
                is_truncated = Some(match decoded.as_ref() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        return Err(StorageError::InvalidResponse(
                            "S3 truncation flag must be true or false".to_owned(),
                        ));
                    }
                });
            }
            Event::Eof => break,
            _ => {}
        }
    }
    match is_truncated {
        Some(true) if next_cursor.is_none() => Err(StorageError::InvalidResponse(
            "truncated S3 object list has no continuation token".to_owned(),
        )),
        Some(false) => Ok(ObjectListPage {
            keys,
            next_cursor: None,
        }),
        Some(true) => Ok(ObjectListPage { keys, next_cursor }),
        None => Err(StorageError::InvalidResponse(
            "S3 object list has no truncation flag".to_owned(),
        )),
    }
}

/// 保守地拒绝存储桶策略中的匿名或公开授权。
///
/// S3 策略可通过不止 `Principal: "*" + Action: "s3:GetObject"` 一种形式表达公开访问。
/// 尤其是，带有 `NotPrincipal` 的 `Allow` 语句会向除列出主体外的所有人授权，而 `NotAction`
/// 也可能间接授予读取权限。RyFrame 的私有文件约定不需要这两种形式，因此采取失败即拒绝策略，
/// 而不在此复刻完整的 IAM 策略求值器。
fn policy_allows_public_access(policy: &Value) -> bool {
    let statements = match policy.get("Statement") {
        Some(Value::Array(statements)) => statements.iter().collect::<Vec<_>>(),
        Some(statement @ Value::Object(_)) => vec![statement],
        _ => return false,
    };
    statements.into_iter().any(|statement| {
        statement.get("Effect").and_then(Value::as_str) == Some("Allow")
            && (statement.get("NotPrincipal").is_some()
                || value_contains_wildcard(statement.get("Principal")))
    })
}

fn value_contains_wildcard(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(value)) => value == "*",
        Some(Value::Array(values)) => values
            .iter()
            .any(|value| value_contains_wildcard(Some(value))),
        Some(Value::Object(values)) => values
            .values()
            .any(|value| value_contains_wildcard(Some(value))),
        _ => false,
    }
}

fn normalize_endpoint(endpoint: &str, use_ssl: bool) -> StorageResult<Url> {
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

fn empty_payload_hash() -> String {
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

fn s3_request_result_label(result: &Result<Response, reqwest::Error>) -> &'static str {
    match result {
        Ok(response) if response.status().is_success() => "success",
        Ok(response) if response.status().is_client_error() => "client_error",
        Ok(response) if response.status().is_server_error() => "server_error",
        Ok(_) => "other_http",
        Err(_) => "transport_error",
    }
}

async fn service_error(operation: &'static str, response: Response) -> StorageError {
    let status = response.status().as_u16();
    let mut message = response.text().await.unwrap_or_default();
    message.truncate(2048);
    StorageError::Service {
        operation,
        status,
        message,
    }
}
