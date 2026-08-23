use std::{error::Error, fmt, path::Path, time::Duration};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactStoreErrorKind {
    InvalidLocation,
    NotFound,
    Misconfigured,
    Rejected,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactStoreError {
    kind: ArtifactStoreErrorKind,
    message: String,
}

impl ArtifactStoreError {
    pub fn new(kind: ArtifactStoreErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub const fn kind(&self) -> ArtifactStoreErrorKind {
        self.kind
    }
}

impl fmt::Display for ArtifactStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ArtifactStoreError {}

#[async_trait::async_trait]
pub trait ArtifactStore: Send + Sync {
    fn late_put_completion_bound(&self) -> Duration {
        Duration::from_secs(30)
    }

    async fn readiness(&self, bucket: &str) -> Result<(), ArtifactStoreError>;

    async fn ensure_bucket(&self, bucket: &str) -> Result<(), ArtifactStoreError>;

    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), ArtifactStoreError>;

    async fn put_file(
        &self,
        bucket: &str,
        key: &str,
        path: &Path,
        content_type: &str,
        sha256_hex: Option<&str>,
    ) -> Result<(), ArtifactStoreError>;

    async fn get(&self, bucket: &str, key: &str) -> Result<Vec<u8>, ArtifactStoreError>;

    async fn delete(&self, bucket: &str, key: &str) -> Result<(), ArtifactStoreError>;
}
