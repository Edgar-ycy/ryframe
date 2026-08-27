use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
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
        path: String,
    },
    Failed(String),
}

pub(crate) struct SourceWatcher {
    receiver: Receiver<WatchEvent>,
    revision: Arc<AtomicU64>,
    _watcher: RecommendedWatcher,
}

impl SourceWatcher {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let root = root.to_path_buf();
        let callback_root = root.clone();
        let (sender, receiver) = mpsc::channel();
        let revision = Arc::new(AtomicU64::new(0));
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
                if let Some(path) = first_relevant_path(&callback_root, &event.paths) {
                    let revision = SourceRevision::from_value(
                        callback_revision.fetch_add(1, Ordering::AcqRel) + 1,
                    );
                    let _ = sender.send(WatchEvent::BackendChanged { revision, path });
                }
            })?;

        watcher.watch(&root, RecursiveMode::NonRecursive)?;
        for relative in [".cargo", "crates", "config", "catalog", "locales", "xtask"] {
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
        SourceRevision::from_value(self.revision.load(Ordering::Acquire))
    }

    pub(crate) fn is_superseded(&self, expected: SourceRevision) -> bool {
        self.current_revision() > expected
    }

    pub(crate) fn drain_changes(
        &self,
        initial_revision: SourceRevision,
        initial_path: String,
        quiet_period: Duration,
    ) -> Result<ChangeBatch> {
        let mut revision = initial_revision;
        let mut paths = BTreeSet::from([initial_path]);
        loop {
            match self.receiver.recv_timeout(quiet_period) {
                Ok(WatchEvent::BackendChanged {
                    revision: next_revision,
                    path,
                }) => {
                    revision = revision.max(next_revision);
                    paths.insert(path);
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

fn first_relevant_path(root: &Path, paths: &[PathBuf]) -> Option<String> {
    paths.iter().find_map(|path| {
        let relative = path.strip_prefix(root).unwrap_or(path);
        let normalized = relative.to_string_lossy().replace('\\', "/");
        is_backend_watch_path(&normalized).then_some(normalized)
    })
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
