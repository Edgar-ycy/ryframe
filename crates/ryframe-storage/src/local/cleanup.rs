use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime},
};

use tempfile::{Builder, NamedTempFile};

use super::{LocalObjectStorage, STAGING_FILE_PREFIX, STAGING_FILE_SUFFIX, STAGING_STALE_AFTER};
use crate::{StorageError, StorageResult};

const CLEANUP_INTERVAL: Duration = Duration::from_secs(15 * 60);
pub(super) const MAX_SCANNED_PER_RUN: usize = 256;
pub(super) const MAX_REMOVED_PER_RUN: usize = 128;

#[derive(Debug, Default)]
struct BucketCleanupState {
    in_progress: bool,
    rerun_requested: bool,
    last_completed: Option<Instant>,
    next_due: Option<Instant>,
    #[cfg(test)]
    started_runs: u64,
    #[cfg(test)]
    completed_runs: u64,
    #[cfg(test)]
    max_run_scanned: usize,
    #[cfg(test)]
    max_run_removed: usize,
    #[cfg(test)]
    total_scanned: usize,
    #[cfg(test)]
    total_removed: usize,
    #[cfg(test)]
    total_directories_opened: usize,
}

#[derive(Debug, Default)]
pub(super) struct CleanupSchedule {
    buckets: Mutex<HashMap<String, BucketCleanupState>>,
}

impl CleanupSchedule {
    fn buckets(&self) -> MutexGuard<'_, HashMap<String, BucketCleanupState>> {
        self.buckets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn claim(&self, bucket: &str, force: bool) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets();
        let state = buckets.entry(bucket.to_owned()).or_default();
        if state.in_progress {
            if force {
                state.rerun_requested = true;
            }
            return false;
        }
        if !force && state.next_due.is_some_and(|next_due| now < next_due) {
            return false;
        }
        state.in_progress = true;
        #[cfg(test)]
        {
            state.started_runs += 1;
        }
        true
    }

    fn finish_run(
        &self,
        bucket: &str,
        report: Option<&CleanupReport>,
        cursor_has_more: bool,
    ) -> NextCleanupRun {
        #[cfg(not(test))]
        let _ = report;
        let completed_at = Instant::now();
        let mut buckets = self.buckets();
        let state = buckets.entry(bucket.to_owned()).or_default();
        #[cfg(test)]
        {
            state.completed_runs += 1;
            if let Some(report) = report {
                state.max_run_scanned = state.max_run_scanned.max(report.scanned_entries);
                state.max_run_removed = state.max_run_removed.max(report.removed_files);
                state.total_scanned += report.scanned_entries;
                state.total_removed += report.removed_files;
                state.total_directories_opened += report.directories_opened;
            }
        }

        if cursor_has_more {
            #[cfg(test)]
            {
                state.started_runs += 1;
            }
            return NextCleanupRun::Continue;
        }
        if state.rerun_requested {
            state.rerun_requested = false;
            #[cfg(test)]
            {
                state.started_runs += 1;
            }
            return NextCleanupRun::Restart;
        }

        state.in_progress = false;
        state.last_completed = Some(completed_at);
        state.next_due = completed_at
            .checked_add(CLEANUP_INTERVAL)
            .or(Some(completed_at));
        NextCleanupRun::Done
    }

    fn abort_run(&self, bucket: &str) {
        let completed_at = Instant::now();
        let mut buckets = self.buckets();
        let state = buckets.entry(bucket.to_owned()).or_default();
        state.in_progress = false;
        state.rerun_requested = false;
        state.last_completed = Some(completed_at);
        state.next_due = completed_at
            .checked_add(CLEANUP_INTERVAL)
            .or(Some(completed_at));
    }

    #[cfg(test)]
    pub(super) fn test_run_metrics(&self, bucket: &str) -> (usize, usize, usize, usize, usize) {
        let buckets = self.buckets();
        let Some(state) = buckets.get(bucket) else {
            return (0, 0, 0, 0, 0);
        };
        (
            state.max_run_scanned,
            state.max_run_removed,
            state.total_scanned,
            state.total_removed,
            state.total_directories_opened,
        )
    }

    #[cfg(test)]
    pub(super) fn test_snapshot(&self, bucket: &str) -> (bool, u64, u64, bool, bool) {
        let buckets = self.buckets();
        let Some(state) = buckets.get(bucket) else {
            return (false, 0, 0, false, false);
        };
        (
            state.in_progress,
            state.started_runs,
            state.completed_runs,
            state.last_completed.is_some(),
            state.next_due.is_some(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NextCleanupRun {
    Continue,
    Restart,
    Done,
}

struct CleanupCompletionGuard {
    schedule: Arc<CleanupSchedule>,
    bucket: String,
    finished: bool,
}

impl CleanupCompletionGuard {
    fn finish(mut self, report: Option<&CleanupReport>, cursor_has_more: bool) -> NextCleanupRun {
        self.finished = true;
        self.schedule
            .finish_run(&self.bucket, report, cursor_has_more)
    }
}

impl Drop for CleanupCompletionGuard {
    fn drop(&mut self) {
        if !self.finished {
            self.schedule.abort_run(&self.bucket);
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct ActiveStagingRegistry {
    paths: Mutex<HashSet<PathBuf>>,
}

impl ActiveStagingRegistry {
    fn paths(&self) -> MutexGuard<'_, HashSet<PathBuf>> {
        self.paths
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn create(
        self: &Arc<Self>,
        staging_directory: &Path,
    ) -> StorageResult<ActiveStagingFile> {
        // Hold the registry lock across create+register so cleanup can never
        // observe an active staging file in an unregistered window.
        let mut paths = self.paths();
        let file = Builder::new()
            .prefix(STAGING_FILE_PREFIX)
            .suffix(STAGING_FILE_SUFFIX)
            .rand_bytes(16)
            .tempfile_in(staging_directory)
            .map_err(|source| StorageError::Io {
                operation: "create local object staging file",
                source,
            })?;
        file.as_file()
            .try_lock()
            .map_err(|error| StorageError::Io {
                operation: "lock local object staging file",
                source: match error {
                    std::fs::TryLockError::Error(source) => source,
                    std::fs::TryLockError::WouldBlock => std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "new staging file is unexpectedly locked",
                    ),
                },
            })?;
        let path = file.path().to_path_buf();
        paths.insert(path.clone());
        drop(paths);
        Ok(ActiveStagingFile {
            file: Some(file),
            path,
            registry: self.clone(),
        })
    }

    fn contains(&self, path: &Path) -> bool {
        self.paths().contains(path)
    }
}

pub(super) struct ActiveStagingFile {
    file: Option<NamedTempFile>,
    path: PathBuf,
    registry: Arc<ActiveStagingRegistry>,
}

impl ActiveStagingFile {
    pub(super) fn as_file(&self) -> StorageResult<&std::fs::File> {
        self.file
            .as_ref()
            .map(NamedTempFile::as_file)
            .ok_or_else(|| StorageError::Io {
                operation: "access local object staging file",
                source: std::io::Error::other("active staging file is unavailable"),
            })
    }

    pub(super) fn persist(mut self, final_path: &Path) -> StorageResult<()> {
        // Persist is synchronous and the registry lock makes publication and
        // deregistration one indivisible operation from cleanup's perspective.
        let mut paths = self.registry.paths();
        let Some(file) = self.file.take() else {
            paths.remove(&self.path);
            drop(paths);
            return Err(StorageError::Io {
                operation: "publish local object staging file",
                source: std::io::Error::other("active staging file is unavailable"),
            });
        };
        let result = file.persist(final_path);
        let source = match result {
            Ok(file) => {
                drop(file);
                None
            }
            Err(error) => {
                let source = error.error;
                drop(error.file);
                Some(source)
            }
        };
        paths.remove(&self.path);
        drop(paths);
        match source {
            Some(source) => Err(StorageError::Io {
                operation: "publish local object staging file",
                source,
            }),
            None => Ok(()),
        }
    }
}

impl Drop for ActiveStagingFile {
    fn drop(&mut self) {
        let mut paths = self.registry.paths();
        // NamedTempFile performs best-effort deletion. Keep the path registered
        // until its handle has closed and cleanup has been attempted.
        drop(self.file.take());
        paths.remove(&self.path);
    }
}

#[derive(Debug, Default)]
pub(super) struct CleanupReport {
    pub(super) scanned_entries: usize,
    pub(super) removed_files: usize,
    pub(super) skipped_active: usize,
    pub(super) directories_opened: usize,
    failures: usize,
    first_error: Option<String>,
}

impl CleanupReport {
    fn record_failure(&mut self, context: &str, error: &impl std::fmt::Display) {
        self.failures += 1;
        if self.first_error.is_none() {
            self.first_error = Some(format!("{context}: {error}"));
        }
    }
}

struct CleanupCursor {
    staging_directory: PathBuf,
    entries: tokio::fs::ReadDir,
    directory_open_count_pending: bool,
    now: SystemTime,
    stale_after: Duration,
}

struct CleanupRunOutcome {
    report: CleanupReport,
    cursor: Option<CleanupCursor>,
}

impl LocalObjectStorage {
    #[cfg(not(test))]
    fn staging_stale_after(&self) -> Duration {
        STAGING_STALE_AFTER
    }

    #[cfg(test)]
    fn staging_stale_after(&self) -> Duration {
        self.test_stale_after.unwrap_or(STAGING_STALE_AFTER)
    }

    pub(super) fn trigger_staging_cleanup(&self, bucket: &str, force: bool) {
        if !self.cleanup_schedule.claim(bucket, force) {
            return;
        }

        self.clone()
            .spawn_claimed_cleanup_run(bucket.to_owned(), None);
    }

    fn spawn_claimed_cleanup_run(self, bucket: String, cursor: Option<CleanupCursor>) {
        let completion = CleanupCompletionGuard {
            schedule: self.cleanup_schedule.clone(),
            bucket: bucket.clone(),
            finished: false,
        };
        let _cleanup_task = tokio::spawn(async move {
            let outcome = match cursor {
                Some(cursor) => self.cleanup_staging_run(cursor).await,
                None => {
                    let cursor = self
                        .open_cleanup_cursor(&bucket, SystemTime::now(), self.staging_stale_after())
                        .await;
                    match cursor {
                        Ok(cursor) => self.cleanup_staging_run(cursor).await,
                        Err(error) => Err(error),
                    }
                }
            };

            match outcome {
                Ok(CleanupRunOutcome { report, cursor }) => {
                    if report.failures > 0 {
                        tracing::warn!(
                            bucket,
                            scanned_entries = report.scanned_entries,
                            removed_files = report.removed_files,
                            skipped_active = report.skipped_active,
                            directories_opened = report.directories_opened,
                            failures = report.failures,
                            first_error = ?report.first_error,
                            "local staging cleanup completed with recoverable errors"
                        );
                    } else if report.removed_files > 0 {
                        tracing::info!(
                            bucket,
                            scanned_entries = report.scanned_entries,
                            removed_files = report.removed_files,
                            skipped_active = report.skipped_active,
                            directories_opened = report.directories_opened,
                            "removed stale local object staging files"
                        );
                    }

                    let next = completion.finish(Some(&report), cursor.is_some());
                    match next {
                        NextCleanupRun::Continue => self.spawn_claimed_cleanup_run(bucket, cursor),
                        NextCleanupRun::Restart => {
                            self.spawn_claimed_cleanup_run(bucket, None);
                        }
                        NextCleanupRun::Done => {}
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        bucket,
                        error = %error,
                        "local staging cleanup failed; object storage remains available"
                    );
                    if completion.finish(None, false) == NextCleanupRun::Restart {
                        self.spawn_claimed_cleanup_run(bucket, None);
                    }
                }
            }
        });
    }

    async fn open_cleanup_cursor(
        &self,
        bucket: &str,
        now: SystemTime,
        stale_after: Duration,
    ) -> StorageResult<CleanupCursor> {
        let staging_directory = self.canonical_staging_directory(bucket, false).await?;
        let entries = tokio::fs::read_dir(&staging_directory)
            .await
            .map_err(|source| StorageError::Io {
                operation: "read local staging directory",
                source,
            })?;
        Ok(CleanupCursor {
            staging_directory,
            entries,
            directory_open_count_pending: true,
            now,
            stale_after,
        })
    }

    #[cfg(test)]
    pub(super) async fn cleanup_staging_directory_at(
        &self,
        bucket: &str,
        now: SystemTime,
        stale_after: Duration,
    ) -> StorageResult<CleanupReport> {
        let cursor = self.open_cleanup_cursor(bucket, now, stale_after).await?;
        self.cleanup_staging_run(cursor)
            .await
            .map(|outcome| outcome.report)
    }

    async fn cleanup_staging_run(
        &self,
        mut cursor: CleanupCursor,
    ) -> StorageResult<CleanupRunOutcome> {
        let mut report = CleanupReport {
            directories_opened: usize::from(cursor.directory_open_count_pending),
            ..CleanupReport::default()
        };
        cursor.directory_open_count_pending = false;
        let mut reached_end = false;

        while report.scanned_entries < MAX_SCANNED_PER_RUN
            && report.removed_files < MAX_REMOVED_PER_RUN
        {
            let entry = match cursor.entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => {
                    reached_end = true;
                    break;
                }
                Err(error) => {
                    report.record_failure("read local staging entry", &error);
                    reached_end = true;
                    break;
                }
            };
            report.scanned_entries += 1;
            let path = entry.path();
            let matches_staging_name = entry.file_name().to_str().is_some_and(|name| {
                name.starts_with(STAGING_FILE_PREFIX) && name.ends_with(STAGING_FILE_SUFFIX)
            });
            if !matches_staging_name {
                continue;
            }
            let file_type = match entry.file_type().await {
                Ok(file_type) => file_type,
                Err(error) => {
                    report.record_failure("inspect local staging entry type", &error);
                    continue;
                }
            };
            if !file_type.is_file() || file_type.is_symlink() {
                continue;
            }
            if self.active_staging.contains(&path) {
                report.skipped_active += 1;
                continue;
            }
            let metadata = match tokio::fs::symlink_metadata(&path).await {
                Ok(metadata) if metadata.file_type().is_file() => metadata,
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    report.record_failure("inspect local staging candidate", &error);
                    continue;
                }
            };
            if !metadata
                .modified()
                .ok()
                .and_then(|modified| cursor.now.duration_since(modified).ok())
                .is_some_and(|age| age >= cursor.stale_after)
            {
                continue;
            }
            let resolved = match tokio::fs::canonicalize(&path).await {
                Ok(path)
                    if path.parent() == Some(cursor.staging_directory.as_path())
                        && path.starts_with(&cursor.staging_directory) =>
                {
                    path
                }
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    report.record_failure("resolve local staging candidate", &error);
                    continue;
                }
            };
            if self.active_staging.contains(&resolved) {
                report.skipped_active += 1;
                continue;
            }

            let locked_file = match tokio::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&resolved)
                .await
            {
                Ok(file) => file.into_std().await,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    report.record_failure("open local staging candidate for locking", &error);
                    continue;
                }
            };
            match locked_file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => {
                    report.skipped_active += 1;
                    continue;
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    report.record_failure("lock local staging candidate", &error);
                    continue;
                }
            }

            let metadata = match tokio::fs::symlink_metadata(&resolved).await {
                Ok(metadata) if metadata.file_type().is_file() => metadata,
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    report.record_failure("recheck local staging candidate", &error);
                    continue;
                }
            };
            if !metadata
                .modified()
                .ok()
                .and_then(|modified| cursor.now.duration_since(modified).ok())
                .is_some_and(|age| age >= cursor.stale_after)
            {
                continue;
            }
            match tokio::fs::remove_file(&resolved).await {
                Ok(()) => report.removed_files += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    report.record_failure("remove stale local staging file", &error);
                }
            }
        }

        Ok(CleanupRunOutcome {
            report,
            cursor: (!reached_end).then_some(cursor),
        })
    }
}
