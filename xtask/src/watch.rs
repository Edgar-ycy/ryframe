use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    time::Duration,
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SourceRevision(u64);

impl SourceRevision {
    pub(crate) const fn from_value(value: u64) -> Self {
        Self(value)
    }

    pub(crate) fn value(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangeBatch {
    pub(crate) revision: SourceRevision,
    pub(crate) paths: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WatchEvent {
    BackendChanged {
        revision: SourceRevision,
        paths: BTreeSet<String>,
    },
    Failed(String),
}

pub(crate) struct SourceWatcher {
    receiver: Receiver<WatchEvent>,
    revision: Arc<SourceRevisionTracker>,
    _watcher: RecommendedWatcher,
}

#[derive(Default)]
pub(crate) struct SourceRevisionTracker {
    revision: AtomicU64,
    event_order: Mutex<()>,
}

impl SourceRevisionTracker {
    pub(crate) fn current_revision(&self) -> SourceRevision {
        SourceRevision::from_value(self.revision.load(Ordering::Acquire))
    }

    /// 与事件代次分配共享串行化点。返回 `true` 后到达的事件进入下一轮队列，
    /// 不再撤销已经获准进入正式端口切换临界区的候选。
    pub(crate) fn final_revision_fence(&self, expected: SourceRevision) -> bool {
        let _order = self.lock_event_order();
        self.current_revision() == expected
    }

    fn lock_event_order(&self) -> MutexGuard<'_, ()> {
        self.event_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn record_paths(
        &self,
        root: &Path,
        event_paths: &[PathBuf],
    ) -> Option<(SourceRevision, BTreeSet<String>)> {
        let _order = self.lock_event_order();
        let paths = relevant_paths(root, event_paths);
        (!paths.is_empty()).then(|| (self.next_revision(), paths))
    }

    fn next_revision(&self) -> SourceRevision {
        SourceRevision::from_value(self.revision.fetch_add(1, Ordering::AcqRel) + 1)
    }
}

impl SourceWatcher {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let root = root.to_path_buf();
        let callback_root = root.clone();
        let (sender, receiver) = mpsc::channel();
        let revision = Arc::new(SourceRevisionTracker::default());
        let callback_revision = Arc::clone(&revision);
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let event = match result {
                    Ok(event) => event,
                    Err(error) => {
                        let _ = sender.send(WatchEvent::Failed(format!("源码监听失败：{error}")));
                        return;
                    }
                };
                if let Some((revision, paths)) =
                    callback_revision.record_paths(&callback_root, &event.paths)
                {
                    let _ = sender.send(WatchEvent::BackendChanged { revision, paths });
                }
            })?;

        watcher.watch(&root, RecursiveMode::NonRecursive)?;
        for relative in [
            ".cargo", "crates", "config", "catalog", "locales", "vendor", "xtask",
        ] {
            let path = root.join(relative);
            if path.is_dir() {
                watcher.watch(&path, RecursiveMode::Recursive)?;
            }
        }
        Ok(Self {
            receiver,
            revision,
            _watcher: watcher,
        })
    }

    pub(crate) fn recv_timeout(&self, timeout: Duration) -> Result<Option<WatchEvent>> {
        match self.receiver.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err("源码监听器意外停止".into()),
        }
    }

    pub(crate) fn current_revision(&self) -> SourceRevision {
        self.revision.current_revision()
    }

    pub(crate) fn is_superseded(&self, expected: SourceRevision) -> bool {
        self.current_revision() > expected
    }

    pub(crate) fn final_revision_fence(&self, expected: SourceRevision) -> bool {
        self.revision.final_revision_fence(expected)
    }

    pub(crate) fn drain_changes(
        &self,
        initial_revision: SourceRevision,
        initial_paths: BTreeSet<String>,
        quiet_period: Duration,
    ) -> Result<ChangeBatch> {
        let mut revision = initial_revision;
        let mut paths = initial_paths;
        loop {
            match self.receiver.recv_timeout(quiet_period) {
                Ok(WatchEvent::BackendChanged {
                    revision: next_revision,
                    paths: next_paths,
                }) => {
                    revision = revision.max(next_revision);
                    paths.extend(next_paths);
                }
                Ok(WatchEvent::Failed(error)) => return Err(error.into()),
                Err(RecvTimeoutError::Timeout) => return Ok(ChangeBatch { revision, paths }),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("源码监听器意外停止".into());
                }
            }
        }
    }
}

pub(crate) fn relevant_paths(root: &Path, paths: &[PathBuf]) -> BTreeSet<String> {
    paths
        .iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(root).unwrap_or(path);
            let normalized = relative.to_string_lossy().replace('\\', "/");
            is_backend_watch_path(&normalized).then_some(normalized)
        })
        .collect()
}

pub(crate) fn is_backend_watch_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    if normalized.is_empty()
        || normalized == "target"
        || normalized.starts_with("target/")
        || normalized == ".git"
        || normalized.starts_with(".git/")
        || normalized == ".local-tests"
        || normalized.starts_with(".local-tests/")
        || normalized.starts_with("logs/")
    {
        return false;
    }
    if matches!(
        normalized.as_str(),
        "cargo.toml" | "cargo.lock" | "build.rs" | "rust-toolchain" | "rust-toolchain.toml"
    ) {
        return true;
    }
    if normalized.starts_with("crates/") {
        return normalized.ends_with(".rs")
            || normalized.ends_with("cargo.toml")
            || normalized.ends_with("build.rs");
    }
    if normalized.starts_with("xtask/") {
        return normalized.ends_with(".rs")
            || normalized.ends_with("cargo.toml")
            || normalized.ends_with("build.rs");
    }
    if normalized.starts_with("vendor/") {
        return matches!(
            Path::new(&normalized)
                .extension()
                .and_then(|value| value.to_str()),
            Some("rs" | "toml" | "lock" | "c" | "cc" | "cpp" | "h" | "hpp" | "s" | "asm")
        );
    }
    if normalized.starts_with(".cargo/") {
        return matches!(
            Path::new(&normalized)
                .extension()
                .and_then(|value| value.to_str()),
            Some("toml")
        );
    }
    (normalized.starts_with("config/")
        || normalized.starts_with("catalog/")
        || normalized.starts_with("locales/"))
        && matches!(
            Path::new(&normalized)
                .extension()
                .and_then(|value| value.to_str()),
            Some("toml" | "json")
        )
}
