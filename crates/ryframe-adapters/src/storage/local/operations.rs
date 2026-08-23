use std::{path::Path, time::Duration};

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::{
    ObjectListPage, ObjectStorage, StorageError, StorageOperation, StorageResult,
    trace_storage_operation, validate_list_request,
};
use super::LocalObjectStorage;

#[async_trait]
impl ObjectStorage for LocalObjectStorage {
    fn late_put_completion_bound(&self) -> Duration {
        // 异步工作仅写入私有暂存文件。发布操作是在同一文件系统内同步重命名，
        // 因而取消操作不会导致最终对象键在之后才出现。
        Duration::ZERO
    }

    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        trace_storage_operation("local", StorageOperation::Put, async {
            self.write_bytes(bucket, key, data, true).await
        })
        .await
    }

    async fn put_control(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        trace_storage_operation("local", StorageOperation::Put, async {
            self.write_bytes(bucket, key, data, false).await
        })
        .await
    }

    async fn put_file(
        &self,
        bucket: &str,
        key: &str,
        path: &Path,
        _content_type: &str,
        _sha256_hex: Option<&str>,
    ) -> StorageResult<()> {
        trace_storage_operation("local", StorageOperation::Put, async {
            let mut reader =
                tokio::fs::File::open(path)
                    .await
                    .map_err(|source| StorageError::Io {
                        operation: "open local upload source",
                        source,
                    })?;
            let metadata = reader.metadata().await.map_err(|source| StorageError::Io {
                operation: "inspect local upload source",
                source,
            })?;
            if !metadata.is_file() {
                return Err(StorageError::InvalidLocation(
                    "upload source must be a regular file".to_owned(),
                ));
            }

            let (bucket_root, segments, staging) = self.create_staging(bucket, key, true).await?;
            let mut writer = Self::staging_writer(&staging)?;
            tokio::io::copy(&mut reader, &mut writer)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "copy local upload source",
                    source,
                })?;
            writer.flush().await.map_err(|source| StorageError::Io {
                operation: "flush local object staging file",
                source,
            })?;
            drop(writer);
            self.publish_staging(&bucket_root, &segments, staging).await
        })
        .await
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        trace_storage_operation("local", StorageOperation::Get, async {
            let segments = self.validate_location(bucket, key)?;
            let bucket_root = self.canonical_bucket_directory(bucket, false).await?;
            let resolved = self.resolve_existing_path(&bucket_root, &segments).await?;
            tokio::fs::read(resolved)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "read local object",
                    source,
                })
        })
        .await
    }

    async fn get_bounded(
        &self,
        bucket: &str,
        key: &str,
        max_bytes: usize,
    ) -> StorageResult<Vec<u8>> {
        trace_storage_operation("local", StorageOperation::Get, async {
            if max_bytes == 0 {
                return Err(StorageError::InvalidLocation(
                    "bounded object read limit must be greater than zero".to_owned(),
                ));
            }
            let segments = self.validate_location(bucket, key)?;
            let bucket_root = self.canonical_bucket_directory(bucket, false).await?;
            let resolved = self.resolve_existing_path(&bucket_root, &segments).await?;
            let mut reader =
                tokio::fs::File::open(resolved)
                    .await
                    .map_err(|source| StorageError::Io {
                        operation: "open bounded local object",
                        source,
                    })?;
            let metadata = reader.metadata().await.map_err(|source| StorageError::Io {
                operation: "inspect bounded local object",
                source,
            })?;
            if metadata.len() > max_bytes as u64 {
                return Err(StorageError::InvalidResponse(
                    "object exceeds bounded read limit".to_owned(),
                ));
            }
            let mut data = Vec::with_capacity(metadata.len() as usize);
            reader
                .read_to_end(&mut data)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "read bounded local object",
                    source,
                })?;
            if data.len() > max_bytes {
                return Err(StorageError::InvalidResponse(
                    "object changed beyond bounded read limit".to_owned(),
                ));
            }
            Ok(data)
        })
        .await
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
        trace_storage_operation("local", StorageOperation::Delete, async {
            let segments = self.validate_location(bucket, key)?;
            let bucket_root = match self.canonical_bucket_directory(bucket, false).await {
                Ok(path) => path,
                Err(StorageError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error),
            };
            let resolved = match self.resolve_existing_path(&bucket_root, &segments).await {
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
        })
        .await
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
        trace_storage_operation("local", StorageOperation::Exists, async {
            let segments = self.validate_location(bucket, key)?;
            let bucket_root = match self.canonical_bucket_directory(bucket, false).await {
                Ok(path) => path,
                Err(StorageError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    return Ok(false);
                }
                Err(error) => return Err(error),
            };
            match self.resolve_existing_path(&bucket_root, &segments).await {
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
        })
        .await
    }

    async fn list_page(
        &self,
        bucket: &str,
        prefix: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> StorageResult<ObjectListPage> {
        trace_storage_operation("local", StorageOperation::List, async {
            validate_list_request(bucket, prefix, cursor, limit)?;
            let Some(prefix_root) = self.resolve_prefix_directory(bucket, prefix).await? else {
                return Ok(ObjectListPage {
                    keys: Vec::new(),
                    next_cursor: None,
                });
            };
            self.collect_page_candidates(prefix_root, prefix, cursor, limit)
                .await
        })
        .await
    }

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        trace_storage_operation("local", StorageOperation::EnsureBucket, async {
            self.canonical_bucket_directory(bucket, true).await?;
            self.canonical_staging_directory(bucket, true).await?;
            // 启动清理由尽力而为：无效存储桶路径仍会在上方以失败即拒绝方式处理；
            // 清理 I/O 失败只记录日志，不会使原本可用的本地存储失效。
            self.trigger_staging_cleanup(bucket, true);
            Ok(())
        })
        .await
    }

    async fn readiness_check(&self, bucket: &str) -> StorageResult<()> {
        trace_storage_operation("local", StorageOperation::Readiness, async {
            // 健康探针不得创建存储根目录、存储桶、暂存目录或探测对象。已配置的
            // 存储桶缺失或不可读即表示就绪检查失败。
            self.canonical_bucket_directory(bucket, false).await?;
            Ok(())
        })
        .await
    }
}
