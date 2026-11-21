use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    time::Duration,
};

use crate::Result;

#[path = "watch/dispatch.rs"]
mod dispatch;
#[path = "watch/inputs.rs"]
mod inputs;
#[path = "watch/subscriptions.rs"]
mod subscriptions;
#[cfg(test)]
#[path = "watch/testing.rs"]
pub(crate) mod testing;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WatchInputRoots {
    pub(crate) config_dir: PathBuf,
    pub(crate) locales_dir: PathBuf,
}

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
    sender: mpsc::Sender<WatchEvent>,
    revision: Arc<SourceRevisionTracker>,
    mapping: Arc<Mutex<inputs::Mapping>>,
    subscriptions: Arc<Mutex<Option<subscriptions::Subscriptions>>>,
    pipeline: Option<dispatch::Pipeline>,
}

#[derive(Default)]
pub(crate) struct SourceRevisionTracker {
    revision: AtomicU64,
    event_order: Mutex<()>,
    raw_arrivals: AtomicU64,
    applied_raw: AtomicU64,
    failed: AtomicBool,
    health: Mutex<Arc<AtomicBool>>,
}

impl SourceRevisionTracker {
    pub(crate) fn current_revision(&self) -> SourceRevision {
        SourceRevision::from_value(self.revision.load(Ordering::Acquire))
    }

    /// 与事件代次分配共享串行化点。返回 `true` 后到达的事件进入下一轮队列，
    /// 不再撤销已经获准进入正式端口切换临界区的候选。
    pub(crate) fn final_revision_fence(&self, expected: SourceRevision) -> bool {
        let _order = self.lock_event_order();
        self.fence_under_gate(expected)
    }

    fn fence_under_gate(&self, expected: SourceRevision) -> bool {
        // arrival 是最后一次读取；此后到达的事件属于下一轮。
        let ready = !self.failed.load(Ordering::SeqCst)
            && !self.health_failed()
            && self.current_revision() == expected;
        let applied = self.applied_raw.load(Ordering::SeqCst);
        let arrivals = self.raw_arrivals.load(Ordering::SeqCst);
        ready && arrivals != u64::MAX && applied == arrivals
    }

    fn is_superseded(&self, expected: SourceRevision) -> bool {
        let _gate = self.lock_event_order();
        self.superseded_under_gate(expected)
    }

    fn superseded_under_gate(&self, expected: SourceRevision) -> bool {
        // 先读 applied；若分发恰在两次读取间完成，也不会漏掉新逻辑代次。
        let applied = self.applied_raw.load(Ordering::SeqCst);
        let invalid = self.failed.load(Ordering::SeqCst)
            || self.health_failed()
            || self.current_revision() > expected;
        let arrivals = self.raw_arrivals.load(Ordering::SeqCst);
        invalid || arrivals == u64::MAX || applied != arrivals
    }

    fn health_failed(&self) -> bool {
        self.health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .load(Ordering::SeqCst)
    }

    fn lock_event_order(&self) -> MutexGuard<'_, ()> {
        self.event_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    pub(crate) fn record_paths(
        &self,
        root: &Path,
        event_paths: &[PathBuf],
    ) -> Option<(SourceRevision, BTreeSet<String>)> {
        let _order = self.lock_event_order();
        let paths = relevant_paths(root, event_paths);
        if paths.is_empty() {
            None
        } else {
            self.next_revision().map(|revision| (revision, paths))
        }
    }

    fn next_revision(&self) -> Option<SourceRevision> {
        match self
            .revision
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_add(1)
            }) {
            Ok(previous) => Some(SourceRevision::from_value(previous + 1)),
            Err(_) => {
                self.failed.store(true, Ordering::SeqCst);
                None
            }
        }
    }
}

impl SourceWatcher {
    #[cfg(test)]
    pub(crate) fn new(root: &Path) -> Result<Self> {
        Self::construct(root, None, SourceRevision::from_value(0))
    }

    pub(crate) fn new_with_inputs(
        root: &Path,
        inputs: WatchInputRoots,
        initial_revision: SourceRevision,
    ) -> Result<Self> {
        Self::construct(root, Some(inputs), initial_revision)
    }

    fn construct(
        root: &Path,
        inputs: Option<WatchInputRoots>,
        seed: SourceRevision,
    ) -> Result<Self> {
        let mapping = inputs::Mapping::new(root, inputs)?;
        let root = mapping.root.clone();
        let (sender, receiver) = mpsc::channel();
        let revision = Arc::new(SourceRevisionTracker {
            revision: AtomicU64::new(seed.value()),
            ..Default::default()
        });
        let mapping = Arc::new(Mutex::new(mapping));
        let subscriptions = Arc::new(Mutex::new(None));
        let pipeline = dispatch::Pipeline::start(
            Arc::clone(&revision),
            Arc::clone(&mapping),
            Arc::clone(&subscriptions),
            sender.clone(),
        )?;
        let result = Self {
            receiver,
            sender,
            revision,
            mapping,
            subscriptions,
            pipeline: Some(pipeline),
        };
        {
            let _gate = result.revision.lock_event_order();
            let mapping = result
                .mapping
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let ingress = result
                .pipeline
                .as_ref()
                .ok_or("源码监听分发器未建立；请重新启动")?
                .ingress();
            let backend = subscriptions::Subscriptions::new(&root, &mapping, ingress)?;
            backend.ensure_healthy("发布初始监听实例前")?;
            *result
                .revision
                .health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = backend.health();
            *result
                .subscriptions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(backend);
        }
        Ok(result)
    }

    pub(crate) fn update_inputs(&mut self, inputs: WatchInputRoots) -> Result<()> {
        self.replace_inputs_with(inputs, |mapping, ingress| {
            subscriptions::Subscriptions::new(&mapping.root, mapping, ingress)
        })
    }

    fn replace_inputs_with(
        &mut self,
        inputs: WatchInputRoots,
        prepare: impl FnOnce(
            &inputs::Mapping,
            dispatch::Ingress,
        ) -> Result<subscriptions::Subscriptions>,
    ) -> Result<()> {
        let _gate = self.revision.lock_event_order();
        let mut current = self
            .mapping
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = inputs::Mapping::new(&current.root, Some(inputs))?;
        let changed = current.changed_paths(&next);
        if !changed.is_empty() && self.current_revision().value() == u64::MAX {
            self.revision.failed.store(true, Ordering::SeqCst);
            return Err(
                "输入重绑失败：逻辑代次已耗尽；未发布新映射，请停止监督器并检查保存代次后重新启动"
                    .into(),
            );
        }
        let mut backend = self
            .subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = backend.as_ref().ok_or("源码监听已关闭；请重新启动监督器")?;
        previous.ensure_healthy("准备输入重绑前")?;
        if self.revision.failed.load(Ordering::SeqCst) {
            return Err("输入重绑失败：raw 分发流已不可信；请停止监督器并重新启动".into());
        }
        let ingress = self
            .pipeline
            .as_ref()
            .ok_or("监听分发器已关闭；请重新启动监督器")?
            .ingress();
        let prepared = prepare(&next, ingress);
        previous.ensure_healthy("发布输入重绑前，旧监听实例")?;
        let prepared = prepared?;
        prepared.ensure_healthy("发布输入重绑前，新监听实例")?;
        *self
            .revision
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = prepared.health();
        *current = next;
        let previous = backend.replace(prepared);
        if !changed.is_empty() {
            dispatch::publish(&self.revision, &self.sender, changed);
        }
        // 销毁旧整个实例，不取消新实例的单项订阅，也不在 gate 内等待分发线程。
        drop(previous);
        Ok(())
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
        self.revision.is_superseded(expected)
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

impl Drop for SourceWatcher {
    fn drop(&mut self) {
        // 先停止原生监听；释放所有锁后再关闭队列并等待分发线程。
        let backend = self
            .subscriptions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        drop(backend);
        drop(self.pipeline.take());
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
