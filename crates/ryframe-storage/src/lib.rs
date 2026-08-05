//! 对象存储端口与生产后端。

mod local;
mod s3;
mod signing;

use std::{future::Future, time::Duration};

use async_trait::async_trait;
pub use local::LocalObjectStorage;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
pub use s3::{S3Config, S3ObjectStorage};
use tracing::Instrument;

const OBJECT_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'!')
    .add(b'"')
    .add(b'#')
    .add(b'$')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'(')
    .add(b')')
    .add(b'*')
    .add(b'+')
    .add(b',')
    .add(b'/')
    .add(b':')
    .add(b';')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'?')
    .add(b'@')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

pub type StorageResult<T> = Result<T, StorageError>;

/// 对象存储 span 使用的固定操作集合，禁止将存储桶、对象键、端点或签名写入属性。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StorageOperation {
    Put,
    Get,
    Delete,
    Exists,
    Readiness,
    EnsureBucket,
    BucketHead,
    BucketCreate,
    BucketSetAcl,
    BucketGetPolicy,
    ObjectHead,
}

impl StorageOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Put => "PUT",
            Self::Get => "GET",
            Self::Delete => "DELETE",
            Self::Exists => "EXISTS",
            Self::Readiness => "READINESS",
            Self::EnsureBucket => "ENSURE_BUCKET",
            Self::BucketHead => "BUCKET_HEAD",
            Self::BucketCreate => "BUCKET_CREATE",
            Self::BucketSetAcl => "BUCKET_SET_ACL",
            Self::BucketGetPolicy => "BUCKET_GET_POLICY",
            Self::ObjectHead => "OBJECT_HEAD",
        }
    }
}

pub(crate) async fn trace_storage_operation<T>(
    backend: &'static str,
    operation: StorageOperation,
    future: impl Future<Output = StorageResult<T>>,
) -> StorageResult<T> {
    let span = storage_operation_span(backend, operation);
    let result = future.instrument(span.clone()).await;
    span.record("storage.result", storage_result_label(&result));
    result
}

pub(crate) fn storage_operation_span(
    backend: &'static str,
    operation: StorageOperation,
) -> tracing::Span {
    tracing::info_span!(
        "storage.operation",
        otel.name = operation.as_str(),
        otel.kind = "client",
        storage.backend = backend,
        storage.operation = operation.as_str(),
        storage.result = tracing::field::Empty,
    )
}

fn storage_result_label<T>(result: &StorageResult<T>) -> &'static str {
    if result.is_ok() { "success" } else { "error" }
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("invalid storage location: {0}")]
    InvalidLocation(String),
    #[error("invalid storage configuration: {0}")]
    Configuration(String),
    #[error("{operation} failed: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("object storage request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("{operation} failed with HTTP {status}: {message}")]
    Service {
        operation: &'static str,
        status: u16,
        message: String,
    },
    #[error("request signing failed: {0}")]
    Signing(String),
    #[error("object storage readiness check failed: {0}")]
    Readiness(String),
}

/// 在不暴露具体后端的前提下上传、下载、删除和定位对象。
#[async_trait]
pub trait ObjectStorage: Send + Sync {
    /// 返回的 future 被取消后，后端仍可能提交 PUT 的最长时间。上传清理墓碑会保留超过该时长，
    /// 以便第二次删除能够捕获远端延迟完成。
    ///
    /// 具有更大上限的实现必须覆盖此方法。内置 S3 客户端的总请求超时为 30 秒；本地写入
    /// 不会脱离远端操作。
    fn late_put_completion_bound(&self) -> Duration {
        Duration::from_secs(30)
    }

    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> StorageResult<()>;

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>>;

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()>;

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool>;

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        validate_bucket(bucket)
    }

    /// 在不改变存储状态的情况下检查已配置存储桶是否可访问。创建存储桶和强制策略应在启动阶段完成，
    /// 而非放入频繁调用的就绪探针。
    async fn readiness_check(&self, bucket: &str) -> StorageResult<()> {
        validate_bucket(bucket)?;
        // 对通用实现而言，刻意只查询元数据已足够。内置后端会以存储桶级检查覆盖它，
        // 从而也能检测到存储桶被删除的情况。
        let _ = self.exists(bucket, ".ryframe-readiness/probe").await?;
        Ok(())
    }
}

fn validate_bucket(bucket: &str) -> StorageResult<()> {
    let bytes = bucket.as_bytes();
    let valid_edge = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    if !(3..=63).contains(&bytes.len())
        || !bytes.first().is_some_and(|byte| valid_edge(*byte))
        || !bytes.last().is_some_and(|byte| valid_edge(*byte))
        || !bytes
            .iter()
            .all(|byte| valid_edge(*byte) || matches!(byte, b'.' | b'-'))
        || bucket.contains("..")
        || bucket.contains(".-")
        || bucket.contains("-.")
        || bucket.parse::<std::net::IpAddr>().is_ok()
    {
        return Err(StorageError::InvalidLocation(format!(
            "bucket '{bucket}' must be 3-63 lowercase letters, digits, dots, or hyphens"
        )));
    }
    Ok(())
}

fn key_segments(key: &str) -> StorageResult<Vec<&str>> {
    if key.is_empty() || key.len() > 1024 || key.starts_with('/') || key.ends_with('/') {
        return Err(StorageError::InvalidLocation(
            "object key must contain 1-1024 bytes and be relative".to_owned(),
        ));
    }
    if key.contains('\\') || key.chars().any(char::is_control) {
        return Err(StorageError::InvalidLocation(
            "object key contains a forbidden character".to_owned(),
        ));
    }

    let segments: Vec<_> = key.split('/').collect();
    if segments
        .iter()
        .any(|segment| segment.is_empty() || matches!(*segment, "." | ".."))
    {
        return Err(StorageError::InvalidLocation(
            "object key contains an invalid path segment".to_owned(),
        ));
    }
    Ok(segments)
}

fn encoded_segment(value: &str) -> String {
    utf8_percent_encode(value, OBJECT_SEGMENT_ENCODE_SET).to_string()
}
