use std::sync::Arc;

use ryframe_adapters::storage::{ObjectStorage, StorageError};
use ryframe_application::ports::files::{
    ArtifactStore, ArtifactStoreError, ArtifactStoreErrorKind, ArtifactStoreFuture,
};

struct ObjectStorageBridge {
    storage: Arc<dyn ObjectStorage>,
}

impl ArtifactStore for ObjectStorageBridge {
    fn late_put_completion_bound(&self) -> std::time::Duration {
        self.storage.late_put_completion_bound()
    }

    fn readiness<'a>(&'a self, bucket: &'a str) -> ArtifactStoreFuture<'a, ()> {
        Box::pin(async move {
            self.storage
                .readiness_check(bucket)
                .await
                .map_err(map_storage_error)
        })
    }

    fn ensure_bucket<'a>(&'a self, bucket: &'a str) -> ArtifactStoreFuture<'a, ()> {
        Box::pin(async move {
            self.storage
                .ensure_bucket(bucket)
                .await
                .map_err(map_storage_error)
        })
    }

    fn put<'a>(
        &'a self,
        bucket: &'a str,
        key: &'a str,
        data: &'a [u8],
        content_type: &'a str,
    ) -> ArtifactStoreFuture<'a, ()> {
        Box::pin(async move {
            self.storage
                .put(bucket, key, data, content_type)
                .await
                .map_err(map_storage_error)
        })
    }

    fn put_file<'a>(
        &'a self,
        bucket: &'a str,
        key: &'a str,
        path: &'a std::path::Path,
        content_type: &'a str,
        sha256_hex: Option<&'a str>,
    ) -> ArtifactStoreFuture<'a, ()> {
        Box::pin(async move {
            self.storage
                .put_file(bucket, key, path, content_type, sha256_hex)
                .await
                .map_err(map_storage_error)
        })
    }

    fn get<'a>(&'a self, bucket: &'a str, key: &'a str) -> ArtifactStoreFuture<'a, Vec<u8>> {
        Box::pin(async move {
            self.storage
                .get(bucket, key)
                .await
                .map_err(map_storage_error)
        })
    }

    fn delete<'a>(&'a self, bucket: &'a str, key: &'a str) -> ArtifactStoreFuture<'a, ()> {
        Box::pin(async move {
            self.storage
                .delete(bucket, key)
                .await
                .map_err(map_storage_error)
        })
    }
}

pub fn application_store(storage: Arc<dyn ObjectStorage>) -> Arc<dyn ArtifactStore> {
    Arc::new(ObjectStorageBridge { storage })
}

fn map_storage_error(error: StorageError) -> ArtifactStoreError {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_errors_are_mapped_to_stable_application_kinds() {
        let not_found = map_storage_error(StorageError::Service {
            operation: "GET",
            status: 404,
            message: "missing".into(),
        });
        let unavailable = map_storage_error(StorageError::Service {
            operation: "PUT",
            status: 503,
            message: "busy".into(),
        });
        let misconfigured = map_storage_error(StorageError::Unsupported("operation".into()));

        assert_eq!(not_found.kind(), ArtifactStoreErrorKind::NotFound);
        assert_eq!(unavailable.kind(), ArtifactStoreErrorKind::Unavailable);
        assert_eq!(misconfigured.kind(), ArtifactStoreErrorKind::Misconfigured);
    }
}
