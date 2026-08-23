mod cleanup;
mod operations;
mod validation;

use std::{
    collections::BinaryHeap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use tokio::io::AsyncWriteExt;

use super::{ObjectListPage, StorageError, StorageResult, validate_bucket};
use cleanup::{ActiveStagingFile, ActiveStagingRegistry, CleanupSchedule};
use validation::{is_link_or_reparse, local_key_segments};

const STAGING_FILE_PREFIX: &str = ".ryframe-staging-";
const STAGING_FILE_SUFFIX: &str = ".part";
pub(super) const STAGING_DIRECTORY_NAME: &str = ".ryframe-staging";
// 保留一天可避免与缓慢写入竞争，并限制异常终止后的临时文件。
const STAGING_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
#[derive(Clone, Debug)]
pub struct LocalObjectStorage {
    base_dir: PathBuf,
    cleanup_schedule: Arc<CleanupSchedule>,
    active_staging: Arc<ActiveStagingRegistry>,
}

impl LocalObjectStorage {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            cleanup_schedule: Arc::new(CleanupSchedule::default()),
            active_staging: Arc::new(ActiveStagingRegistry::default()),
        }
    }

    fn validate_location<'a>(&self, bucket: &str, key: &'a str) -> StorageResult<Vec<&'a str>> {
        validate_bucket(bucket)?;
        local_key_segments(key)
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

    async fn canonical_bucket_directory(
        &self,
        bucket: &str,
        create: bool,
    ) -> StorageResult<PathBuf> {
        validate_bucket(bucket)?;
        let base = self.canonical_base(create).await?;
        let path = base.join(bucket);
        self.ensure_real_directory(&path, create, "local bucket")
            .await?;
        let resolved = tokio::fs::canonicalize(&path)
            .await
            .map_err(|source| StorageError::Io {
                operation: "resolve local bucket",
                source,
            })?;
        if resolved.parent() != Some(base.as_path()) {
            return Err(StorageError::InvalidLocation(
                "local bucket escapes the storage root".to_owned(),
            ));
        }
        Ok(resolved)
    }

    async fn canonical_staging_directory(
        &self,
        bucket: &str,
        create: bool,
    ) -> StorageResult<PathBuf> {
        let bucket_root = self.canonical_bucket_directory(bucket, create).await?;
        self.canonical_staging_directory_in(&bucket_root, create)
            .await
    }

    async fn canonical_staging_directory_in(
        &self,
        bucket_root: &Path,
        create: bool,
    ) -> StorageResult<PathBuf> {
        let path = bucket_root.join(STAGING_DIRECTORY_NAME);
        self.ensure_real_directory(&path, create, "local staging directory")
            .await?;
        let resolved = tokio::fs::canonicalize(&path)
            .await
            .map_err(|source| StorageError::Io {
                operation: "resolve local staging directory",
                source,
            })?;
        if resolved.parent() != Some(bucket_root) || !resolved.starts_with(bucket_root) {
            return Err(StorageError::InvalidLocation(
                "local staging namespace escapes the bucket root".to_owned(),
            ));
        }
        Ok(resolved)
    }

    async fn ensure_real_directory(
        &self,
        path: &Path,
        create: bool,
        context: &'static str,
    ) -> StorageResult<()> {
        let metadata = match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) => metadata,
            Err(source) if create && source.kind() == std::io::ErrorKind::NotFound => {
                match tokio::fs::create_dir(path).await {
                    Ok(()) => {}
                    Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(source) => {
                        return Err(StorageError::Io {
                            operation: "create local storage directory",
                            source,
                        });
                    }
                }
                tokio::fs::symlink_metadata(path)
                    .await
                    .map_err(|source| StorageError::Io {
                        operation: "inspect created local storage directory",
                        source,
                    })?
            }
            Err(source) => {
                return Err(StorageError::Io {
                    operation: "inspect local storage directory",
                    source,
                });
            }
        };
        if is_link_or_reparse(&metadata) || !metadata.is_dir() {
            return Err(StorageError::InvalidLocation(format!(
                "{context} must be a real directory, not a link or reparse point"
            )));
        }
        Ok(())
    }

    async fn object_path_in_bucket(
        &self,
        bucket_root: &Path,
        segments: &[&str],
        create_parents: bool,
    ) -> StorageResult<PathBuf> {
        let (file_name, parent_segments) = segments.split_last().ok_or_else(|| {
            StorageError::InvalidLocation("object key has no path segments".to_owned())
        })?;
        let reserved_staging = match self
            .canonical_staging_directory_in(bucket_root, false)
            .await
        {
            Ok(path) => Some(path),
            Err(StorageError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                None
            }
            Err(error) => return Err(error),
        };
        let mut parent = bucket_root.to_path_buf();
        for segment in parent_segments {
            let next = parent.join(segment);
            self.ensure_real_directory(&next, create_parents, "object parent directory")
                .await?;
            let resolved =
                tokio::fs::canonicalize(&next)
                    .await
                    .map_err(|source| StorageError::Io {
                        operation: "resolve object parent directory",
                        source,
                    })?;
            if !resolved.starts_with(bucket_root) {
                return Err(StorageError::InvalidLocation(
                    "object parent directory escapes its bucket root".to_owned(),
                ));
            }
            Self::reject_resolved_reserved_path(&resolved, reserved_staging.as_deref())?;
            parent = resolved;
        }
        Ok(parent.join(file_name))
    }

    fn reject_resolved_reserved_path(
        resolved: &Path,
        reserved_staging: Option<&Path>,
    ) -> StorageResult<()> {
        if reserved_staging.is_some_and(|reserved| resolved.starts_with(reserved)) {
            return Err(StorageError::InvalidLocation(
                "object path resolves into the reserved local-storage staging namespace".to_owned(),
            ));
        }
        Ok(())
    }

    async fn reject_link_target(&self, path: &Path) -> StorageResult<()> {
        match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) if is_link_or_reparse(&metadata) => Err(StorageError::InvalidLocation(
                "object path targets a link or reparse point".to_owned(),
            )),
            Ok(metadata) if metadata.is_dir() => Err(StorageError::InvalidLocation(
                "object path targets a directory".to_owned(),
            )),
            Ok(_) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(StorageError::Io {
                operation: "inspect local object path",
                source,
            }),
        }
    }

    async fn prepare_write_path(
        &self,
        bucket_root: &Path,
        segments: &[&str],
    ) -> StorageResult<PathBuf> {
        let path = self
            .object_path_in_bucket(bucket_root, segments, true)
            .await?;
        self.reject_link_target(&path).await?;
        Ok(path)
    }

    async fn validate_publish_path(
        &self,
        bucket_root: &Path,
        segments: &[&str],
    ) -> StorageResult<PathBuf> {
        let path = self
            .object_path_in_bucket(bucket_root, segments, false)
            .await?;
        self.reject_link_target(&path).await?;
        let parent = path.parent().ok_or_else(|| {
            StorageError::InvalidLocation("object path has no parent directory".to_owned())
        })?;
        let resolved_parent =
            tokio::fs::canonicalize(parent)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "resolve object directory before publish",
                    source,
                })?;
        if !resolved_parent.starts_with(bucket_root) {
            return Err(StorageError::InvalidLocation(
                "object path escapes its bucket root".to_owned(),
            ));
        }
        Ok(resolved_parent.join(path.file_name().ok_or_else(|| {
            StorageError::InvalidLocation("object path has no file name".to_owned())
        })?))
    }

    async fn resolve_existing_path(
        &self,
        bucket_root: &Path,
        segments: &[&str],
    ) -> StorageResult<PathBuf> {
        let path = self
            .object_path_in_bucket(bucket_root, segments, false)
            .await?;
        let metadata =
            tokio::fs::symlink_metadata(&path)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "inspect local object path",
                    source,
                })?;
        if is_link_or_reparse(&metadata) {
            return Err(StorageError::InvalidLocation(
                "object path targets a link or reparse point".to_owned(),
            ));
        }
        let resolved = tokio::fs::canonicalize(&path)
            .await
            .map_err(|source| StorageError::Io {
                operation: "resolve local object",
                source,
            })?;
        if !resolved.starts_with(bucket_root) {
            return Err(StorageError::InvalidLocation(
                "object path escapes its bucket root".to_owned(),
            ));
        }
        Ok(resolved)
    }

    async fn create_staging<'key>(
        &self,
        bucket: &str,
        key: &'key str,
        run_cleanup: bool,
    ) -> StorageResult<(PathBuf, Vec<&'key str>, ActiveStagingFile)> {
        let segments = self.validate_location(bucket, key)?;
        let bucket_root = self.canonical_bucket_directory(bucket, true).await?;
        self.prepare_write_path(&bucket_root, &segments).await?;
        let staging_directory = self
            .canonical_staging_directory_in(&bucket_root, true)
            .await?;
        if run_cleanup {
            self.trigger_staging_cleanup(bucket, false);
        }
        let staging = self.active_staging.create(&staging_directory)?;
        Ok((bucket_root, segments, staging))
    }

    async fn write_bytes(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        run_cleanup: bool,
    ) -> StorageResult<()> {
        let (bucket_root, segments, staging) =
            self.create_staging(bucket, key, run_cleanup).await?;
        let mut writer = Self::staging_writer(&staging)?;
        writer
            .write_all(data)
            .await
            .map_err(|source| StorageError::Io {
                operation: "write local object staging file",
                source,
            })?;
        writer.flush().await.map_err(|source| StorageError::Io {
            operation: "flush local object staging file",
            source,
        })?;
        drop(writer);
        self.publish_staging(&bucket_root, &segments, staging).await
    }

    fn staging_writer(staging: &ActiveStagingFile) -> StorageResult<tokio::fs::File> {
        let writer = staging
            .as_file()?
            .try_clone()
            .map_err(|source| StorageError::Io {
                operation: "open local object staging file",
                source,
            })?;
        Ok(tokio::fs::File::from_std(writer))
    }

    async fn publish_staging(
        &self,
        bucket_root: &Path,
        segments: &[&str],
        staging: ActiveStagingFile,
    ) -> StorageResult<()> {
        // 最后一次异步校验完成后直接执行同文件系统原子替换，避免取消后延迟发布。
        let publish_path = self.validate_publish_path(bucket_root, segments).await?;
        staging.persist(&publish_path)
    }

    async fn resolve_prefix_directory(
        &self,
        bucket: &str,
        prefix: &str,
    ) -> StorageResult<Option<PathBuf>> {
        let bucket_root = match self.canonical_bucket_directory(bucket, false).await {
            Ok(path) => path,
            Err(StorageError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let prefix_key = prefix
            .strip_suffix('/')
            .expect("调用前已经校验目录型对象前缀");
        let segments = local_key_segments(prefix_key)?;
        let mut current = bucket_root.clone();
        for segment in segments {
            let candidate = current.join(segment);
            let metadata = match tokio::fs::symlink_metadata(&candidate).await {
                Ok(metadata) => metadata,
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(source) => {
                    return Err(StorageError::Io {
                        operation: "inspect local object prefix",
                        source,
                    });
                }
            };
            if is_link_or_reparse(&metadata) {
                return Err(StorageError::InvalidLocation(
                    "local object prefix traverses a link or reparse point".to_owned(),
                ));
            }
            if !metadata.is_dir() {
                return Ok(None);
            }
            let resolved = tokio::fs::canonicalize(&candidate)
                .await
                .map_err(|source| StorageError::Io {
                    operation: "resolve local object prefix",
                    source,
                })?;
            if !resolved.starts_with(&bucket_root) {
                return Err(StorageError::InvalidLocation(
                    "local object prefix escapes its bucket root".to_owned(),
                ));
            }
            current = resolved;
        }
        Ok(Some(current))
    }

    async fn collect_page_candidates(
        &self,
        prefix_root: PathBuf,
        prefix: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> StorageResult<ObjectListPage> {
        if let Some(cursor) = cursor {
            local_key_segments(cursor)?;
            if !cursor.starts_with(prefix) {
                return Err(StorageError::InvalidLocation(
                    "local object list cursor does not belong to the requested prefix".to_owned(),
                ));
            }
        }

        // 扫描过程中只保留当前游标之后最小的 limit + 1 个键。目录数量可能随存储内容
        // 增长，但结果内存始终受调用方页大小和对象键长度上限约束。
        let allowed_root = prefix_root.clone();
        let mut directories = vec![(prefix_root, prefix.to_owned())];
        let mut candidates = BinaryHeap::with_capacity(limit + 1);
        while let Some((directory, logical_prefix)) = directories.pop() {
            let mut entries =
                tokio::fs::read_dir(&directory)
                    .await
                    .map_err(|source| StorageError::Io {
                        operation: "list local object directory",
                        source,
                    })?;
            while let Some(entry) =
                entries
                    .next_entry()
                    .await
                    .map_err(|source| StorageError::Io {
                        operation: "read local object directory entry",
                        source,
                    })?
            {
                let name = entry.file_name().into_string().map_err(|_| {
                    StorageError::InvalidLocation(
                        "local object path contains a non-Unicode segment".to_owned(),
                    )
                })?;
                if name == STAGING_DIRECTORY_NAME {
                    continue;
                }
                let key = format!("{logical_prefix}{name}");
                local_key_segments(&key)?;
                let metadata =
                    tokio::fs::symlink_metadata(entry.path())
                        .await
                        .map_err(|source| StorageError::Io {
                            operation: "inspect listed local object",
                            source,
                        })?;
                if is_link_or_reparse(&metadata) {
                    return Err(StorageError::InvalidLocation(
                        "listed local object traverses a link or reparse point".to_owned(),
                    ));
                }
                if metadata.is_dir() {
                    let resolved =
                        tokio::fs::canonicalize(entry.path())
                            .await
                            .map_err(|source| StorageError::Io {
                                operation: "resolve listed local object directory",
                                source,
                            })?;
                    if !resolved.starts_with(&allowed_root) {
                        return Err(StorageError::InvalidLocation(
                            "listed local object directory escapes its prefix root".to_owned(),
                        ));
                    }
                    directories.push((resolved, format!("{key}/")));
                    continue;
                }
                if !metadata.is_file() || cursor.is_some_and(|cursor| key.as_str() <= cursor) {
                    continue;
                }
                candidates.push(key);
                if candidates.len() > limit + 1 {
                    candidates.pop();
                }
            }
        }

        let mut keys = candidates.into_sorted_vec();
        let has_more = keys.len() > limit;
        if has_more {
            keys.truncate(limit);
        }
        let next_cursor =
            has_more.then(|| keys.last().expect("存在下一页时当前页必定包含对象").clone());
        Ok(ObjectListPage { keys, next_cursor })
    }
}
