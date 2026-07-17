//! Object storage port and production backends.

mod local;
mod s3;
mod signing;

use async_trait::async_trait;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

pub use local::LocalObjectStorage;
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
}

/// Upload, download, delete, and locate objects without exposing a backend.
#[async_trait]
pub trait ObjectStorage: Send + Sync {
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

    /// Return a direct public URL when one is explicitly configured.
    fn public_url(&self, bucket: &str, key: &str) -> StorageResult<Option<String>>;

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        validate_bucket(bucket)
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

fn public_object_url(base_url: &str, bucket: &str, key: &str) -> StorageResult<String> {
    validate_bucket(bucket)?;
    let key = key_segments(key)?
        .into_iter()
        .map(encoded_segment)
        .collect::<Vec<_>>()
        .join("/");
    Ok(format!(
        "{}/{}/{}",
        base_url.trim_end_matches('/'),
        bucket,
        key
    ))
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

    #[test]
    fn public_urls_encode_each_object_segment() {
        assert_eq!(
            public_object_url("https://cdn.example.com/", "photos", "夏季/photo one.jpg").unwrap(),
            "https://cdn.example.com/photos/%E5%A4%8F%E5%AD%A3/photo%20one.jpg"
        );
    }
}
