use std::{fmt, io::SeekFrom, path::Path, time::Duration};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{StreamExt, stream};
use reqwest::{Body, Method, RequestBuilder, Response, Url};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::Instrument;
mod object_storage;
mod parser;
mod utilities;

pub use parser::parse_list_objects_response;
pub use utilities::normalize_sha256;

use parser::policy_allows_public_access;
use utilities::{
    empty_payload_hash, normalize_endpoint, s3_request_result_label, service_error, transport_error,
};

use super::{
    ObjectListPage, ObjectStorage, StorageError, StorageOperation, StorageResult, key_segments,
    signing::SigV4Signer, storage_operation_span, trace_storage_operation, validate_bucket,
    validate_list_request,
};

const MAX_LIST_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// 路径风格 S3 兼容端点的连接与签名配置。
#[derive(Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub use_ssl: bool,
    pub region: String,
    pub request_timeout_secs: u64,
}

impl fmt::Debug for S3Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field("use_ssl", &self.use_ssl)
            .field("region", &self.region)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .finish()
    }
}

/// 适用于 AWS S3 与 MinIO 的 S3 兼容 HTTP 后端。
pub struct S3ObjectStorage {
    endpoint: Url,
    access_key: String,
    secret_key: String,
    region: String,
    request_timeout: Duration,
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
        if !(1..=300).contains(&config.request_timeout_secs) {
            return Err(StorageError::Configuration(
                "S3 request_timeout_secs must be between 1 and 300".to_owned(),
            ));
        }

        let endpoint = normalize_endpoint(&config.endpoint, config.use_ssl)?;
        let request_timeout = Duration::from_secs(config.request_timeout_secs);
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .build()
            .map_err(transport_error)?;
        Ok(Self {
            endpoint,
            access_key: config.access_key,
            secret_key: config.secret_key,
            region: config.region,
            request_timeout,
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
            _ => Err(service_error("check S3 bucket", response)),
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
        Err(service_error("create S3 bucket", response))
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
            return Err(service_error("set private S3 bucket ACL", response));
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
            return Err(service_error("verify S3 bucket policy", response));
        }
        let policy = response.text().await.map_err(transport_error)?;
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
        let result_label = s3_request_result_label(&result);
        span.record("storage.result", result_label);
        result.map_err(transport_error)
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
            let chunk = chunk.map_err(transport_error)?;
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
