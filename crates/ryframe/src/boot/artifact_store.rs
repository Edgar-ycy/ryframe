use std::sync::Arc;

use ryframe_adapters::storage::{ObjectStorage, StorageError};
use ryframe_application::ports::files::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreErrorKind,
};

struct ObjectStorageBridge {
    storage: Arc<dyn ObjectStorage>,
}

#[async_trait::async_trait]
impl ArtifactStore for ObjectStorageBridge {
    fn late_put_completion_bound(&self) -> std::time::Duration {
        self.storage.late_put_completion_bound()
    }

    async fn readiness(&self, bucket: &str) -> Result<(), ArtifactStoreError> {
        self.storage
            .readiness_check(bucket)
            .await
            .map_err(map_storage_error)
    }

    async fn ensure_bucket(&self, bucket: &str) -> Result<(), ArtifactStoreError> {
        self.storage
            .ensure_bucket(bucket)
            .await
            .map_err(map_storage_error)
    }

    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<(), ArtifactStoreError> {
        self.storage
            .put(bucket, key, data, content_type)
            .await
            .map_err(map_storage_error)
    }

    async fn put_file(
        &self,
        bucket: &str,
        key: &str,
        path: &std::path::Path,
        content_type: &str,
        sha256_hex: Option<&str>,
    ) -> Result<(), ArtifactStoreError> {
        self.storage
            .put_file(bucket, key, path, content_type, sha256_hex)
            .await
            .map_err(map_storage_error)
    }

    async fn get(&self, bucket: &str, key: &str) -> Result<Vec<u8>, ArtifactStoreError> {
        self.storage
            .get(bucket, key)
            .await
            .map_err(map_storage_error)
    }

    async fn delete(&self, bucket: &str, key: &str) -> Result<(), ArtifactStoreError> {
        self.storage
            .delete(bucket, key)
            .await
            .map_err(map_storage_error)
    }
}

pub fn application_store(storage: Arc<dyn ObjectStorage>) -> Arc<dyn ArtifactStore> {
    Arc::new(ObjectStorageBridge { storage })
}

pub fn map_storage_error(error: StorageError) -> ArtifactStoreError {
    let kind = match &error {
        StorageError::InvalidLocation(_) => ArtifactStoreErrorKind::InvalidLocation,
        StorageError::Configuration(_)
        | StorageError::Signing(_)
        | StorageError::Unsupported(_) => ArtifactStoreErrorKind::Misconfigured,
        StorageError::Service { status: 404, .. } => ArtifactStoreErrorKind::NotFound,
        StorageError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound => {
            ArtifactStoreErrorKind::NotFound
        }
        StorageError::Service { status, .. } if *status != 429 && *status < 500 => {
            ArtifactStoreErrorKind::Rejected
        }
        StorageError::Io { .. }
        | StorageError::Transport(_)
        | StorageError::Service { .. }
        | StorageError::Readiness(_)
        | StorageError::InvalidResponse(_) => ArtifactStoreErrorKind::Unavailable,
    };
    ArtifactStoreError::new(kind, error.to_string())
}
