use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

use super::{ObjectStorage, StorageError, StorageResult, key_segments, validate_bucket};
use cleanup::{ActiveStagingRegistry, CleanupSchedule};

mod cleanup;

const STAGING_FILE_PREFIX: &str = ".ryframe-staging-";
const STAGING_FILE_SUFFIX: &str = ".part";
const STAGING_DIRECTORY_NAME: &str = ".ryframe-staging";
// A local in-memory upload should never approach this age. Keeping a full day
// avoids racing a slow writer while bounding artifacts left by crash/kill.
const STAGING_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

fn is_link_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

fn is_windows_device_name(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .trim_end_matches([' ', '.'])
        .to_ascii_lowercase();
    if matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "clock$" | "conin$" | "conout$"
    ) {
        return true;
    }

    let numbered_device = |prefix: &str| {
        stem.strip_prefix(prefix).is_some_and(|number| {
            matches!(
                number,
                "1" | "2"
                    | "3"
                    | "4"
                    | "5"
                    | "6"
                    | "7"
                    | "8"
                    | "9"
                    | "\u{00b9}"
                    | "\u{00b2}"
                    | "\u{00b3}"
            )
        })
    };
    numbered_device("com") || numbered_device("lpt")
}

fn local_key_segments(key: &str) -> StorageResult<Vec<&str>> {
    let segments = key_segments(key)?;
    for segment in &segments {
        if segment.eq_ignore_ascii_case(STAGING_DIRECTORY_NAME) {
            return Err(StorageError::InvalidLocation(
                "object key uses the reserved local-storage staging namespace".to_owned(),
            ));
        }
        if segment.contains(':')
            || segment.ends_with('.')
            || segment.ends_with(' ')
            || is_windows_device_name(segment)
        {
            return Err(StorageError::InvalidLocation(
                "object key contains a segment that is unsafe on Windows".to_owned(),
            ));
        }
    }
    Ok(segments)
}

#[cfg(test)]
#[derive(Debug, Default)]
struct PublishPause {
    reached: std::sync::atomic::AtomicBool,
    released: std::sync::atomic::AtomicBool,
}

/// Process-local filesystem backend.
#[derive(Clone, Debug)]
pub struct LocalObjectStorage {
    base_dir: PathBuf,
    cleanup_schedule: Arc<CleanupSchedule>,
    active_staging: Arc<ActiveStagingRegistry>,
    #[cfg(test)]
    publish_pause: Option<std::sync::Arc<PublishPause>>,
    #[cfg(test)]
    test_stale_after: Option<Duration>,
}

impl LocalObjectStorage {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            cleanup_schedule: Arc::new(CleanupSchedule::default()),
            active_staging: Arc::new(ActiveStagingRegistry::default()),
            #[cfg(test)]
            publish_pause: None,
            #[cfg(test)]
            test_stale_after: None,
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

    #[cfg(test)]
    async fn pause_before_publish(&self) {
        if let Some(pause) = &self.publish_pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::Release);
            while !pause.released.load(std::sync::atomic::Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        }
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
}

#[async_trait]
impl ObjectStorage for LocalObjectStorage {
    fn late_put_completion_bound(&self) -> Duration {
        // Async work only writes the private staging file. Publishing is a
        // synchronous same-filesystem rename, so cancellation can never cause
        // the final object key to appear later.
        Duration::ZERO
    }

    async fn put(
        &self,
        bucket: &str,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> StorageResult<()> {
        let segments = self.validate_location(bucket, key)?;
        let bucket_root = self.canonical_bucket_directory(bucket, true).await?;
        self.prepare_write_path(&bucket_root, &segments).await?;
        let staging_directory = self
            .canonical_staging_directory_in(&bucket_root, true)
            .await?;
        self.trigger_staging_cleanup(bucket, false);
        let staging = self.active_staging.create(&staging_directory)?;
        let writer = staging
            .as_file()?
            .try_clone()
            .map_err(|source| StorageError::Io {
                operation: "open local object staging file",
                source,
            })?;
        let mut writer = tokio::fs::File::from_std(writer);
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

        // Re-check the canonical parent after the asynchronous write. No await
        // occurs after this check in production: persist performs the atomic
        // overwrite synchronously (including MoveFileEx on Windows).
        let publish_path = self.validate_publish_path(&bucket_root, &segments).await?;
        #[cfg(test)]
        self.pause_before_publish().await;

        staging.persist(&publish_path)
    }

    async fn get(&self, bucket: &str, key: &str) -> StorageResult<Vec<u8>> {
        let segments = self.validate_location(bucket, key)?;
        let bucket_root = self.canonical_bucket_directory(bucket, false).await?;
        let resolved = self.resolve_existing_path(&bucket_root, &segments).await?;
        tokio::fs::read(resolved)
            .await
            .map_err(|source| StorageError::Io {
                operation: "read local object",
                source,
            })
    }

    async fn delete(&self, bucket: &str, key: &str) -> StorageResult<()> {
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
    }

    async fn exists(&self, bucket: &str, key: &str) -> StorageResult<bool> {
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
    }

    async fn ensure_bucket(&self, bucket: &str) -> StorageResult<()> {
        self.canonical_bucket_directory(bucket, true).await?;
        self.canonical_staging_directory(bucket, true).await?;
        // Startup cleanup is best-effort: invalid bucket paths still fail
        // closed above, while cleanup I/O failures are logged and do not make
        // otherwise usable local storage unavailable.
        self.trigger_staging_cleanup(bucket, true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::OpenOptions,
        path::Path,
        sync::{Arc, atomic::Ordering},
        time::{Instant, SystemTime},
    };

    use tempfile::TempDir;

    use super::*;

    #[cfg(unix)]
    fn create_directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(windows)]
    fn create_directory_link(target: &Path, link: &Path) {
        if let Err(symlink_error) = std::os::windows::fs::symlink_dir(target, link) {
            // Directory junctions do not require the symbolic-link privilege
            // on older Windows hosts. Keep this test exercising a reparse
            // point even when Developer Mode is disabled.
            let output = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .unwrap_or_else(|command_error| {
                    panic!(
                        "failed to create test directory link ({symlink_error}); junction command failed: {command_error}"
                    )
                });
            assert!(
                output.status.success(),
                "failed to create test directory link ({symlink_error}); junction command output: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    impl LocalObjectStorage {
        fn with_publish_pause(mut self, pause: Arc<PublishPause>) -> Self {
            self.publish_pause = Some(pause);
            self
        }

        fn with_staging_stale_after(mut self, stale_after: Duration) -> Self {
            self.test_stale_after = Some(stale_after);
            self
        }
    }

    async fn wait_for_cleanup_runs(
        storage: &LocalObjectStorage,
        bucket: &str,
        expected_completed: u64,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (in_progress, _, completed, _, _) = storage.cleanup_schedule.test_snapshot(bucket);
            if !in_progress && completed >= expected_completed {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for local staging cleanup"
            );
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn local_backend_round_trips_and_deletes_objects() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path());

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
    async fn local_backend_atomically_overwrites_objects() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path());

        storage
            .put("uploads", "same-key.txt", b"old", "text/plain")
            .await
            .unwrap();
        storage
            .put("uploads", "same-key.txt", b"new-value", "text/plain")
            .await
            .unwrap();

        assert_eq!(
            storage.get("uploads", "same-key.txt").await.unwrap(),
            b"new-value"
        );
        assert_eq!(storage.late_put_completion_bound(), Duration::ZERO);
    }

    #[tokio::test]
    async fn cancellation_before_publish_never_exposes_the_final_key() {
        let directory = TempDir::new().unwrap();
        let pause = Arc::new(PublishPause::default());
        let storage = LocalObjectStorage::new(directory.path())
            .with_publish_pause(pause.clone())
            .with_staging_stale_after(Duration::ZERO);
        let task_storage = storage.clone();
        let upload = tokio::spawn(async move {
            task_storage
                .put(
                    "uploads",
                    "cancelled.txt",
                    b"complete staging body",
                    "text/plain",
                )
                .await
        });

        while !pause.reached.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }

        assert!(!storage.exists("uploads", "cancelled.txt").await.unwrap());
        let staging_directory = directory
            .path()
            .join("uploads")
            .join(STAGING_DIRECTORY_NAME);
        let entries = std::fs::read_dir(&staging_directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1, "expected one private staging file");
        assert!(entries[0].starts_with(STAGING_FILE_PREFIX));
        assert!(entries[0].ends_with(STAGING_FILE_SUFFIX));
        let reserved_key = format!("{STAGING_DIRECTORY_NAME}/{}", entries[0]);
        assert!(storage.exists("uploads", &reserved_key).await.is_err());
        assert!(storage.get("uploads", &reserved_key).await.is_err());

        let cleanup = storage
            .cleanup_staging_directory_at("uploads", SystemTime::now(), Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(cleanup.removed_files, 0);
        assert_eq!(cleanup.skipped_active, 1);
        assert!(staging_directory.join(&entries[0]).exists());

        upload.abort();
        assert!(upload.await.unwrap_err().is_cancelled());

        for _ in 0..32 {
            assert!(!storage.exists("uploads", "cancelled.txt").await.unwrap());
            tokio::task::yield_now().await;
        }
        assert!(
            std::fs::read_dir(staging_directory)
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn independent_instance_cleanup_cannot_remove_an_active_upload() {
        let directory = TempDir::new().unwrap();
        let pause = Arc::new(PublishPause::default());
        let writer_storage = LocalObjectStorage::new(directory.path())
            .with_publish_pause(pause.clone())
            .with_staging_stale_after(Duration::ZERO);
        let cleaner_storage =
            LocalObjectStorage::new(directory.path()).with_staging_stale_after(Duration::ZERO);
        let task_storage = writer_storage.clone();
        let upload = tokio::spawn(async move {
            task_storage
                .put(
                    "uploads",
                    "cross-instance.txt",
                    b"complete body",
                    "text/plain",
                )
                .await
        });

        while !pause.reached.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }

        let staging_directory = directory
            .path()
            .join("uploads")
            .join(STAGING_DIRECTORY_NAME);
        let staging_path = std::fs::read_dir(&staging_directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(STAGING_FILE_PREFIX) && name.ends_with(STAGING_FILE_SUFFIX)
                    })
            })
            .expect("active upload must have a private staging file");

        let report = cleaner_storage
            .cleanup_staging_directory_at("uploads", SystemTime::now(), Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(report.removed_files, 0);
        assert_eq!(report.skipped_active, 1);
        assert!(staging_path.exists());

        pause.released.store(true, Ordering::Release);
        upload.await.unwrap().unwrap();
        assert_eq!(
            cleaner_storage
                .get("uploads", "cross-instance.txt")
                .await
                .unwrap(),
            b"complete body"
        );
        assert!(!staging_path.exists());
    }

    #[tokio::test]
    async fn independent_instance_cleanup_waits_for_a_stable_file_lock() {
        let directory = TempDir::new().unwrap();
        let owner_storage = LocalObjectStorage::new(directory.path());
        let cleaner_storage = LocalObjectStorage::new(directory.path());
        let staging_directory = owner_storage
            .canonical_staging_directory("uploads", true)
            .await
            .unwrap();
        let locked_artifact = staging_directory.join(".ryframe-staging-separate-process.part");
        std::fs::write(&locked_artifact, b"upload still in progress").unwrap();
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&locked_artifact)
            .unwrap();
        lock.try_lock().unwrap();

        let report = cleaner_storage
            .cleanup_staging_directory_at("uploads", SystemTime::now(), Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(report.removed_files, 0);
        assert_eq!(report.skipped_active, 1);
        assert!(locked_artifact.exists());

        drop(lock);
        let report = cleaner_storage
            .cleanup_staging_directory_at("uploads", SystemTime::now(), Duration::ZERO)
            .await
            .unwrap();
        assert_eq!(report.removed_files, 1);
        assert_eq!(report.skipped_active, 0);
        assert!(!locked_artifact.exists());
    }

    #[tokio::test]
    async fn ensure_bucket_removes_only_stale_crash_artifacts_from_reserved_directory() {
        let directory = TempDir::new().unwrap();
        let bucket = directory.path().join("uploads");
        let staging_directory = bucket.join(STAGING_DIRECTORY_NAME);
        let ordinary_nested = bucket.join("2026").join("07");
        std::fs::create_dir_all(&staging_directory).unwrap();
        std::fs::create_dir_all(&ordinary_nested).unwrap();

        let crash_artifact = staging_directory.join(".ryframe-staging-process-kill.part");
        let wrong_suffix = staging_directory.join(".ryframe-staging-keep.tmp");
        let wrong_prefix = staging_directory.join("customer-upload.part");
        let matching_directory = staging_directory.join(".ryframe-staging-directory.part");
        let ordinary_nested_file = ordinary_nested.join(".ryframe-staging-user-object.part");
        std::fs::write(&crash_artifact, b"incomplete upload after process kill").unwrap();
        std::fs::write(&wrong_suffix, b"application data").unwrap();
        std::fs::write(&wrong_prefix, b"application data").unwrap();
        std::fs::create_dir(&matching_directory).unwrap();
        std::fs::write(&ordinary_nested_file, b"ordinary object data").unwrap();

        let storage =
            LocalObjectStorage::new(directory.path()).with_staging_stale_after(Duration::ZERO);
        storage.ensure_bucket("uploads").await.unwrap();
        wait_for_cleanup_runs(&storage, "uploads", 1).await;

        assert!(!crash_artifact.exists());
        assert!(wrong_suffix.exists());
        assert!(wrong_prefix.exists());
        assert!(matching_directory.is_dir());
        assert!(ordinary_nested_file.exists());
    }

    #[tokio::test]
    async fn staging_cleanup_preserves_young_files() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path());
        storage.ensure_bucket("uploads").await.unwrap();
        wait_for_cleanup_runs(&storage, "uploads", 1).await;
        let staging = directory
            .path()
            .join("uploads")
            .join(STAGING_DIRECTORY_NAME)
            .join(".ryframe-staging-active-writer.part");
        std::fs::write(&staging, b"active upload").unwrap();

        let report = storage
            .cleanup_staging_directory_at("uploads", SystemTime::now(), STAGING_STALE_AFTER)
            .await
            .unwrap();

        assert_eq!(report.removed_files, 0);
        assert!(staging.exists());
    }

    #[tokio::test]
    async fn first_put_triggers_opportunistic_staging_cleanup() {
        let directory = TempDir::new().unwrap();
        let bucket = directory.path().join("uploads");
        let staging_directory = bucket.join(STAGING_DIRECTORY_NAME);
        std::fs::create_dir_all(&staging_directory).unwrap();
        let crash_artifact = staging_directory.join(".ryframe-staging-previous-process.part");
        std::fs::write(&crash_artifact, b"abandoned staging body").unwrap();
        let storage =
            LocalObjectStorage::new(directory.path()).with_staging_stale_after(Duration::ZERO);

        storage
            .put("uploads", "current.txt", b"current upload", "text/plain")
            .await
            .unwrap();
        wait_for_cleanup_runs(&storage, "uploads", 1).await;

        assert!(!crash_artifact.exists());
        assert_eq!(
            storage.get("uploads", "current.txt").await.unwrap(),
            b"current upload"
        );
    }

    #[tokio::test]
    async fn concurrent_cleanup_triggers_share_one_per_bucket_run() {
        let directory = TempDir::new().unwrap();
        let storage =
            LocalObjectStorage::new(directory.path()).with_staging_stale_after(Duration::ZERO);
        storage
            .canonical_staging_directory("uploads", true)
            .await
            .unwrap();

        for _ in 0..32 {
            storage.trigger_staging_cleanup("uploads", true);
        }
        let (in_progress, started, completed, _, _) =
            storage.cleanup_schedule.test_snapshot("uploads");
        assert!(in_progress);
        assert_eq!(started, 1);
        assert_eq!(completed, 0);

        wait_for_cleanup_runs(&storage, "uploads", 1).await;
        let (in_progress, started, completed, has_last_completed, has_next_due) =
            storage.cleanup_schedule.test_snapshot("uploads");
        assert!(!in_progress);
        assert_eq!(started, 2, "many force triggers coalesce into one rerun");
        assert_eq!(completed, 2);
        assert!(has_last_completed);
        assert!(has_next_due);

        storage.trigger_staging_cleanup("uploads", false);
        let (_, started, completed, _, _) = storage.cleanup_schedule.test_snapshot("uploads");
        assert_eq!(started, 2, "next_due must be measured from completion");
        assert_eq!(completed, 2);
    }

    #[tokio::test]
    async fn cleanup_runs_are_hard_bounded_and_converge_without_scanning_objects() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path());
        let staging_directory = storage
            .canonical_staging_directory("uploads", true)
            .await
            .unwrap();
        let ordinary_root = directory.path().join("uploads").join("objects");
        for directory_index in 0..10 {
            let ordinary_directory = ordinary_root.join(directory_index.to_string());
            std::fs::create_dir_all(&ordinary_directory).unwrap();
            for file_index in 0..50 {
                std::fs::write(
                    ordinary_directory.join(format!("object-{file_index}.bin")),
                    b"ordinary object",
                )
                .unwrap();
            }
        }
        for index in 0..300 {
            std::fs::write(
                staging_directory.join(format!(".ryframe-staging-fixture-{index}.part")),
                b"crash artifact",
            )
            .unwrap();
            std::fs::write(
                staging_directory.join(format!("customer-object-{index}.bin")),
                b"must not be removed",
            )
            .unwrap();
        }

        let storage = storage.with_staging_stale_after(Duration::ZERO);
        storage.trigger_staging_cleanup("uploads", true);
        wait_for_cleanup_runs(&storage, "uploads", 3).await;

        let (in_progress, started, completed, _, _) =
            storage.cleanup_schedule.test_snapshot("uploads");
        assert!(!in_progress);
        assert_eq!(started, completed);
        assert!(completed >= 3);
        let (max_scanned, max_removed, total_scanned, total_removed, directories_opened) =
            storage.cleanup_schedule.test_run_metrics("uploads");
        assert!(max_scanned <= cleanup::MAX_SCANNED_PER_RUN);
        assert!(max_removed <= cleanup::MAX_REMOVED_PER_RUN);
        assert_eq!(total_scanned, 600);
        assert_eq!(total_removed, 300);
        assert_eq!(
            directories_opened, 1,
            "cursor continuations must reuse the original directory handle"
        );
        let remaining = std::fs::read_dir(staging_directory).unwrap().count();
        assert_eq!(remaining, 300);
        assert!(ordinary_root.join("0").join("object-0.bin").exists());
    }

    #[tokio::test]
    async fn local_keys_reject_reserved_and_windows_unsafe_segments_before_io() {
        let directory = TempDir::new().unwrap();
        let storage_root = directory.path().join("not-created");
        let storage = LocalObjectStorage::new(&storage_root);
        let invalid_keys = [
            ".RYFRAME-STAGING/guess.part",
            "folder/.RyFrAmE-StAgInG/guess.part",
            "folder/name:stream",
            "folder/trailing.",
            "folder/trailing ",
            "CON",
            "con.txt",
            "folder/PRN.json",
            "AUX",
            "nul.tar",
            "CLOCK$",
            "CONIN$.txt",
            "conout$",
            "COM1",
            "com9.log",
            "LPT1",
            "lpt9.log",
            "COM\u{00b9}",
            "com\u{00b2}.txt",
            "LPT\u{00b3}",
        ];

        for key in invalid_keys {
            assert!(
                storage
                    .put("uploads", key, b"must not be written", "text/plain")
                    .await
                    .is_err(),
                "unsafe local key was accepted: {key}"
            );
            assert!(
                !storage_root.exists(),
                "validation for {key} performed filesystem writes"
            );
        }

        for key in ["console.txt", "com10.txt", "lpt0", "auxiliary/data.txt"] {
            assert!(
                local_key_segments(key).is_ok(),
                "safe key was rejected: {key}"
            );
        }
    }

    #[tokio::test]
    async fn post_canonical_check_rejects_every_alias_of_the_staging_directory() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path());
        let staging = storage
            .canonical_staging_directory("uploads", true)
            .await
            .unwrap();
        let bucket_root = storage
            .canonical_bucket_directory("uploads", false)
            .await
            .unwrap();

        // Long names, case aliases, and Windows 8.3 names all converge to this
        // same canonical path. The post-resolution guard therefore does not
        // depend on which textual alias the filesystem accepted.
        assert!(
            LocalObjectStorage::reject_resolved_reserved_path(&staging, Some(&staging)).is_err()
        );
        assert!(
            LocalObjectStorage::reject_resolved_reserved_path(
                &staging.join("guessed.part"),
                Some(&staging),
            )
            .is_err()
        );
        assert!(
            LocalObjectStorage::reject_resolved_reserved_path(
                &bucket_root.join("objects"),
                Some(&staging),
            )
            .is_ok()
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn resolved_windows_directory_alias_cannot_enter_staging() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path());
        storage
            .canonical_staging_directory("uploads", true)
            .await
            .unwrap();
        let bucket_root = storage
            .canonical_bucket_directory("uploads", false)
            .await
            .unwrap();

        // Call below the textual validation layer to prove the filesystem
        // resolution check catches an alternate directory spelling itself.
        let result = storage
            .object_path_in_bucket(&bucket_root, &[".RYFRAME-STAGING", "guessed.part"], false)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn bucket_internal_directory_links_cannot_reach_a_sibling_bucket() {
        let directory = TempDir::new().unwrap();
        let source_bucket = directory.path().join("uploads");
        let sibling_bucket = directory.path().join("archive");
        std::fs::create_dir_all(&source_bucket).unwrap();
        std::fs::create_dir_all(&sibling_bucket).unwrap();
        let sibling_object = sibling_bucket.join("existing.txt");
        std::fs::write(&sibling_object, b"sibling data").unwrap();
        create_directory_link(&sibling_bucket, &source_bucket.join("escape"));
        let storage = LocalObjectStorage::new(directory.path());

        assert!(storage.get("uploads", "escape/existing.txt").await.is_err());
        assert!(
            storage
                .delete("uploads", "escape/existing.txt")
                .await
                .is_err()
        );
        assert!(
            storage
                .exists("uploads", "escape/existing.txt")
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&sibling_object).unwrap(), b"sibling data");

        assert!(
            storage
                .put(
                    "uploads",
                    "escape/new.txt",
                    b"must not cross buckets",
                    "text/plain",
                )
                .await
                .is_err()
        );
        assert!(!sibling_bucket.join("new.txt").exists());
        assert!(
            !source_bucket.join(STAGING_DIRECTORY_NAME).exists(),
            "put created staging state before rejecting the linked segment"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn staging_cleanup_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().unwrap();
        let bucket = directory.path().join("uploads");
        let staging_directory = bucket.join(STAGING_DIRECTORY_NAME);
        let outside = directory.path().join("outside");
        std::fs::create_dir_all(&staging_directory).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let outside_staging = outside.join(".ryframe-staging-outside.part");
        std::fs::write(&outside_staging, b"must not be removed").unwrap();
        symlink(
            &outside_staging,
            staging_directory.join(".ryframe-staging-linked.part"),
        )
        .unwrap();
        let storage =
            LocalObjectStorage::new(directory.path()).with_staging_stale_after(Duration::ZERO);

        storage.ensure_bucket("uploads").await.unwrap();
        wait_for_cleanup_runs(&storage, "uploads", 1).await;

        assert!(outside_staging.exists());
    }

    #[tokio::test]
    async fn local_backend_rejects_traversal_before_io() {
        let directory = TempDir::new().unwrap();
        let storage = LocalObjectStorage::new(directory.path());

        assert!(
            storage
                .put("uploads", "../outside", b"value", "text/plain")
                .await
                .is_err()
        );
        assert!(storage.get("uploads", "C:\\outside").await.is_err());
        assert!(
            storage
                .get("uploads", ".ryframe-staging/guess.part")
                .await
                .is_err()
        );
    }
}
