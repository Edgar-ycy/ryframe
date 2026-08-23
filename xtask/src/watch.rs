use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::Duration,
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WatchEvent {
    BackendChanged(String),
    Failed(String),
}

pub(crate) struct SourceWatcher {
    receiver: Receiver<WatchEvent>,
    _watcher: RecommendedWatcher,
}

impl SourceWatcher {
    pub(crate) fn new(root: &Path) -> Result<Self> {
        let root = root.to_path_buf();
        let callback_root = root.clone();
        let (sender, receiver) = mpsc::channel();
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
                    let _ = sender.send(WatchEvent::BackendChanged(path));
                }
            })?;

        watcher.watch(&root, RecursiveMode::NonRecursive)?;
        for relative in ["crates", "config", "catalog", "locales"] {
            let path = root.join(relative);
            if path.is_dir() {
                watcher.watch(&path, RecursiveMode::Recursive)?;
            }
        }
        Ok(Self {
            receiver,
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

    pub(crate) fn drain_changes(
        &self,
        initial_path: String,
        quiet_period: Duration,
    ) -> Result<WatchEvent> {
        let mut latest = WatchEvent::BackendChanged(initial_path);
        loop {
            match self.receiver.recv_timeout(quiet_period) {
                Ok(WatchEvent::BackendChanged(path)) => {
                    latest = WatchEvent::BackendChanged(path);
                }
                Ok(failed @ WatchEvent::Failed(_)) => return Ok(failed),
                Err(RecvTimeoutError::Timeout) => return Ok(latest),
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
