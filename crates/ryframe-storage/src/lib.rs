//! Object storage port and production backends.

mod local;
mod s3;
mod signing;

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use async_trait::async_trait;
pub use local::LocalObjectStorage;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
pub use s3::{S3Config, S3ObjectStorage};

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
static READINESS_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const READINESS_PAYLOAD: &[u8] = b"ryframe-storage-ready";

pub type StorageResult<T> = Result<T, StorageError>;

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

/// Upload, download, delete, and locate objects without exposing a backend.
#[async_trait]
pub trait ObjectStorage: Send + Sync {
    /// Maximum time a backend may still commit a PUT after the returned future
    /// is cancelled. Upload cleanup tombstones are retained beyond this bound
    /// so a second delete catches late remote completion.
    ///
    /// Implementations with a larger bound must override this method. The
    /// bundled S3 client has a 30-second total request timeout; local writes do
    /// not detach a remote operation.
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

    /// Exercise the same private object operations used by the application.
    /// Bucket creation and policy enforcement happen once during startup; the
    /// readiness probe only writes, reads, and removes a tiny private canary.
    async fn readiness_check(&self, bucket: &str) -> StorageResult<()> {
        validate_bucket(bucket)?;
        let sequence = READINESS_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let key = format!(
            ".ryframe-readiness/{}-{}-{sequence}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        self.put(bucket, &key, READINESS_PAYLOAD, "application/octet-stream")
            .await?;
        let read_result = self.get(bucket, &key).await;
        let delete_result = self.delete(bucket, &key).await;

        let payload = read_result?;
        delete_result?;
        if payload != READINESS_PAYLOAD {
            return Err(StorageError::Readiness(
                "canary content did not round-trip exactly".to_owned(),
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_validation_rejects_unsafe_paths() {
        for key in [
            "../secret",
            "/absolute",
            "folder//file",
            "C:\\secret",
            "folder/./file",
        ] {
            assert!(key_segments(key).is_err(), "unsafe key was accepted: {key}");
        }
        for bucket in [
            "UPPER",
            "a",
            "../bucket",
            "bucket/name",
            "bucket..name",
            "127.0.0.1",
        ] {
            assert!(
                validate_bucket(bucket).is_err(),
                "unsafe bucket was accepted: {bucket}"
            );
        }
    }
}
