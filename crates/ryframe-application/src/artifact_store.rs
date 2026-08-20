use std::{error::Error, fmt, future::Future, path::Path, pin::Pin, time::Duration};

pub type ArtifactStoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ArtifactStoreError>> + Send + 'a>>;

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

pub trait ArtifactStore: Send + Sync {
    fn late_put_completion_bound(&self) -> Duration {
        Duration::from_secs(30)
    }

    fn readiness<'a>(&'a self, bucket: &'a str) -> ArtifactStoreFuture<'a, ()>;

    fn ensure_bucket<'a>(&'a self, bucket: &'a str) -> ArtifactStoreFuture<'a, ()>;

    fn put<'a>(
        &'a self,
        bucket: &'a str,
        key: &'a str,
        data: &'a [u8],
        content_type: &'a str,
    ) -> ArtifactStoreFuture<'a, ()>;

    fn put_file<'a>(
        &'a self,
        bucket: &'a str,
        key: &'a str,
        path: &'a Path,
        content_type: &'a str,
        sha256_hex: Option<&'a str>,
    ) -> ArtifactStoreFuture<'a, ()>;

    fn get<'a>(&'a self, bucket: &'a str, key: &'a str) -> ArtifactStoreFuture<'a, Vec<u8>>;

    fn delete<'a>(&'a self, bucket: &'a str, key: &'a str) -> ArtifactStoreFuture<'a, ()>;
}
