//! 对象存储端口与非 SQL 出站实现。

mod local;
mod s3;
mod scoped;
mod signing;

use std::{future::Future, path::Path, time::Duration};

use async_trait::async_trait;
pub use local::LocalObjectStorage;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
pub use s3::{S3Config, S3ObjectStorage};
pub use scoped::ScopedObjectStorage;
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

/// 单次对象列举或清理允许处理的最大对象数。
pub const MAX_OBJECT_LIST_PAGE_SIZE: usize = 1_000;

/// 有界对象列举结果。`next_cursor` 是后端不透明游标，只能原样用于同一存储桶和前缀。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectListPage {
    pub keys: Vec<String>,
    pub next_cursor: Option<String>,
}

/// 单次精确前缀清理结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrefixDeleteBatch {
    pub deleted_count: usize,
    pub may_have_more: bool,
}

/// 对象存储 span 使用的固定操作集合，禁止将存储桶、对象键、端点或签名写入属性。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StorageOperation {
    Put,
    Get,
    Delete,
    Exists,
    List,
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
            Self::List => "LIST",
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
    #[error("object storage returned an invalid response: {0}")]
    InvalidResponse(String),
    #[error("object storage operation is unsupported: {0}")]
    Unsupported(String),
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

    /// 从文件流式上传对象。`sha256_hex` 可复用调用方已计算的 SHA-256，避免 S3 再读一遍文件。
    ///
    /// 实现必须按块读取文件，不得把完整文件加载到内存。
    async fn put_file(
        &self,
        bucket: &str,
        key: &str,
        path: &Path,
        content_type: &str,
        sha256_hex: Option<&str>,
    ) -> StorageResult<()>;

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>>;

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()>;

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool>;

    /// 按精确目录前缀列举一页对象。前缀必须非空且以 `/` 结尾，页大小必须在
    /// `1..=MAX_OBJECT_LIST_PAGE_SIZE` 内。游标由实现定义，调用方不得解析或跨前缀复用。
    async fn list_page(
        &self,
        bucket: &str,
        prefix: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> StorageResult<ObjectListPage> {
        validate_list_request(bucket, prefix, cursor, limit)?;
        Err(StorageError::Unsupported(
            "bounded object listing is not implemented by this backend".to_owned(),
        ))
    }

    /// 删除精确目录前缀下的一批对象。此操作不接受游标，每次都从前缀首项开始，
    /// 因而调用方可在部分失败后使用相同参数安全重试。
    async fn delete_prefix_batch(
        &self,
        bucket: &str,
        prefix: &str,
        limit: usize,
    ) -> StorageResult<PrefixDeleteBatch> {
        let page = self.list_page(bucket, prefix, None, limit).await?;
        if page.keys.len() > limit || page.keys.iter().any(|key| !key.starts_with(prefix)) {
            return Err(StorageError::InvalidResponse(
                "object list page escaped its requested prefix or limit".to_owned(),
            ));
        }
        let deleted_count = page.keys.len();
        let may_have_more = page.next_cursor.is_some();
        for key in page.keys {
            self.delete(bucket, &key).await?;
        }
        Ok(PrefixDeleteBatch {
            deleted_count,
            may_have_more,
        })
    }

    /// 验证精确目录前缀下已经没有对象，不执行任何写操作。
    async fn prefix_is_empty(&self, bucket: &str, prefix: &str) -> StorageResult<bool> {
        let page = self.list_page(bucket, prefix, None, 1).await?;
        Ok(page.keys.is_empty() && page.next_cursor.is_none())
    }

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

fn validate_object_prefix(prefix: &str) -> StorageResult<()> {
    if prefix.len() > 1024 {
        return Err(StorageError::InvalidLocation(
            "object prefix must not exceed 1024 bytes".to_owned(),
        ));
    }
    let Some(without_separator) = prefix.strip_suffix('/') else {
        return Err(StorageError::InvalidLocation(
            "object prefix must be a non-empty directory prefix ending with '/'".to_owned(),
        ));
    };
    if without_separator.is_empty() {
        return Err(StorageError::InvalidLocation(
            "object prefix must not address the whole bucket".to_owned(),
        ));
    }
    key_segments(without_separator)?;
    Ok(())
}

fn validate_list_request(
    bucket: &str,
    prefix: &str,
    cursor: Option<&str>,
    limit: usize,
) -> StorageResult<()> {
    validate_bucket(bucket)?;
    validate_object_prefix(prefix)?;
    if !(1..=MAX_OBJECT_LIST_PAGE_SIZE).contains(&limit) {
        return Err(StorageError::InvalidLocation(format!(
            "object list limit must be between 1 and {MAX_OBJECT_LIST_PAGE_SIZE}"
        )));
    }
    if cursor.is_some_and(|value| {
        value.is_empty() || value.len() > 4_096 || value.chars().any(char::is_control)
    }) {
        return Err(StorageError::InvalidLocation(
            "object list cursor is invalid".to_owned(),
        ));
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
