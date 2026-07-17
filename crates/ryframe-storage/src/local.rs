use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::{
    ObjectStorage, StorageError, StorageResult, key_segments, public_object_url, validate_bucket,
};

/// Process-local filesystem backend.
#[derive(Clone, Debug)]
pub struct LocalObjectStorage {
    base_dir: PathBuf,
    public_base_url: Option<String>,
}

impl LocalObjectStorage {
    pub fn new(base_dir: impl Into<PathBuf>, public_base_url: &str) -> Self {
        Self {
            base_dir: base_dir.into(),
            public_base_url: (!public_base_url.trim().is_empty())
                .then(|| public_base_url.trim_end_matches('/').to_owned()),
        }
    }

    fn file_path(&self, bucket: &str, key: &str) -> StorageResult<PathBuf> {
        validate_bucket(bucket)?;
        let mut path = self.base_dir.join(bucket);
        for segment in key_segments(key)? {
            path.push(segment);
        }
        Ok(path)
    }

    async fn canonical_base(&self, create: bool) -> StorageResult<PathBuf> {
        if create {
            tokio::fs::create_dir_all(&self.base_dir)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "create local storage root",
                    source,
                })?;
        }
        tokio::fs::canonicalize(&self.base_dir)
            .await
            .map_err(|source| StorageError::Io {
                operation: "resolve local storage root",
                source,
            })
    }

    async fn prepare_write_path(&self, path: &Path) -> StorageResult<()> {
        let parent = path.parent().ok_or_else(|| {
            StorageError::InvalidLocation("object path has no parent directory".to_owned())
        })?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| StorageError::Io {
                operation: "create object directory",
                source,
            })?;

        let base = self.canonical_base(true).await?;
        let resolved_parent =
            tokio::fs::canonicalize(parent)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "resolve object directory",
                    source,
                })?;
        if !resolved_parent.starts_with(&base) {
            return Err(StorageError::InvalidLocation(
                "object path escapes the local storage root".to_owned(),
            ));
        }

        if let Ok(metadata) = tokio::fs::symlink_metadata(path).await
            && metadata.file_type().is_symlink()
        {
            return Err(StorageError::InvalidLocation(
                "object path targets a symbolic link".to_owned(),
            ));
        }
        Ok(())
    }

    async fn resolve_existing_path(&self, path: &Path) -> StorageResult<PathBuf> {
        let metadata =
            tokio::fs::symlink_metadata(path)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "inspect local object path",
                    source,
                })?;
        if metadata.file_type().is_symlink() {
            return Err(StorageError::InvalidLocation(
                "object path targets a symbolic link".to_owned(),
            ));
        }
        let base = self.canonical_base(false).await?;
        let resolved = tokio::fs::canonicalize(path)
            .await
            .map_err(|source| StorageError::Io {
                operation: "resolve local object",
                source,
            })?;
        if !resolved.starts_with(base) {
            return Err(StorageError::InvalidLocation(
                "object path escapes the local storage root".to_owned(),
            ));
        }
        Ok(resolved)
    }
}

#[async_trait]
impl ObjectStorage for LocalObjectStorage {
    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        let path = self.file_path(bucket, key)?;
        self.prepare_write_path(&path).await?;
        tokio::fs::write(path, data)
            .await
            .map_err(|source| StorageError::Io {
                operation: "write local object",
                source,
            })
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        let path = self.file_path(bucket, key)?;
        let resolved = self.resolve_existing_path(&path).await?;
        tokio::fs::read(resolved)
            .await
            .map_err(|source| StorageError::Io {
                operation: "read local object",
                source,
            })
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        let path = self.file_path(bucket, key)?;
        let resolved = match self.resolve_existing_path(&path).await {
            Ok(path) => path,
            Err(StorageError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        tokio::fs::remove_file(resolved)
            .await
            .map_err(|source| StorageError::Io {
                operation: "delete local object",
                source,
            })
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        let path = self.file_path(bucket, key)?;
        match self.resolve_existing_path(&path).await {
            Ok(resolved) => tokio::fs::metadata(resolved)
                .await
                .map(|metadata| metadata.is_file())
                .map_err(|source| StorageError::Io {
                    operation: "inspect local object",
                    source,
                }),
            Err(StorageError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    fn public_url(&self, bucket: &str, key: &str) -> StorageResult<Option<String>> {
        validate_bucket(bucket)?;
        key_segments(key)?;
        self.public_base_url
            .as_deref()
            .map(|base_url| public_object_url(base_url, bucket, key))
            .transpose()
    }

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        validate_bucket(bucket)?;
        let path = self.base_dir.join(bucket);
        tokio::fs::create_dir_all(path)
            .await
            .map_err(|source| StorageError::Io {
                operation: "create local bucket",
                source,
            })?;
        self.canonical_base(false).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn local_backend_round_trips_and_deletes_objects() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path(), "https://cdn.example.com/files");

        storage
            .put("uploads", "2026/example.txt", b"value", "text/plain")
            .await
            .unwrap();
        assert!(storage.exists("uploads", "2026/example.txt").await.unwrap());
        assert_eq!(
            storage.get("uploads", "2026/example.txt").await.unwrap(),
            b"value"
        );
        storage.delete("uploads", "2026/example.txt").await.unwrap();
        assert!(!storage.exists("uploads", "2026/example.txt").await.unwrap());
    }

    #[tokio::test]
    async fn local_backend_rejects_traversal_before_io() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path(), "");

        assert!(
            storage
                .put("uploads", "../outside", b"value", "text/plain")
                .await
                .is_err()
        );
        assert!(storage.get("uploads", "C:\\outside").await.is_err());
        assert_eq!(storage.public_url("uploads", "safe/file").unwrap(), None);
    }
}
