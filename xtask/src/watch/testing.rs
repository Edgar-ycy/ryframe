use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
};

use super::{
    SourceRevision, SourceRevisionTracker, SourceWatcher, WatchEvent,
    dispatch::{Ingress, Pipeline},
    inputs::Mapping,
};

#[path = "testing/cases.rs"]
mod cases;

/// 确定性测试入口：使用真实分发线程与 gate，仅替换原生事件的来源。
pub(crate) struct Harness {
    tracker: Arc<SourceRevisionTracker>,
    receiver: Receiver<WatchEvent>,
    pipeline: Option<Pipeline>,
    root: PathBuf,
}

impl Harness {
    pub(crate) fn new(root: &Path, seed: u64) -> Self {
        let mapping = Mapping::new(root, None).unwrap();
        let root = mapping.root.clone();
        let tracker = Arc::new(SourceRevisionTracker {
            revision: AtomicU64::new(seed),
            ..Default::default()
        });
        let (sender, receiver) = mpsc::channel();
        let pipeline = Pipeline::start(
            Arc::clone(&tracker),
            Arc::new(Mutex::new(mapping)),
            Arc::new(Mutex::new(None)),
            sender,
        )
        .unwrap();
        Self {
            tracker,
            receiver,
            pipeline: Some(pipeline),
            root,
        }
    }

    pub(crate) fn gate(&self) -> MutexGuard<'_, ()> {
        self.tracker.lock_event_order()
    }

    pub(crate) fn fence_under_gate(&self, expected: u64) -> bool {
        self.tracker
            .fence_under_gate(SourceRevision::from_value(expected))
    }

    pub(crate) fn fence(&self, expected: u64) -> bool {
        self.tracker
            .final_revision_fence(SourceRevision::from_value(expected))
    }

    pub(crate) fn superseded(&self, expected: u64) -> bool {
        self.tracker
            .superseded_under_gate(SourceRevision::from_value(expected))
    }

    pub(crate) fn send(&self, paths: &[&str]) {
        let mut event = notify::Event::new(notify::EventKind::Any);
        event.paths = paths.iter().map(|path| self.root.join(path)).collect();
        self.ingress().send(&self.health(), Ok(event));
    }

    pub(crate) fn error(&self) {
        self.ingress().send(&self.health(), Err(
            notify::Error::io(std::io::Error::from_raw_os_error(5)).add_path(self.root.clone())
        ));
    }

    pub(crate) fn barrier(&self) {
        self.pipeline.as_ref().unwrap().barrier();
    }

    pub(crate) fn counts(&self) -> (u64, u64, u64) {
        (
            self.tracker.raw_arrivals.load(Ordering::SeqCst),
            self.tracker.applied_raw.load(Ordering::SeqCst),
            self.tracker.current_revision().value(),
        )
    }

    pub(crate) fn events(&self) -> Vec<WatchEvent> {
        self.receiver.try_iter().collect()
    }

    pub(crate) fn gap(&self, sequence: u64) {
        self.pipeline.as_ref().unwrap().inject_sequence(sequence);
    }

    pub(crate) fn close_with_late_sender(mut self) -> bool {
        let late_sender = self.ingress();
        let finished = self.pipeline.as_ref().unwrap().finished();
        drop(self.pipeline.take());
        let before = self.counts();
        late_sender.send(
            &self.health(),
            Ok(notify::Event::new(notify::EventKind::Any)),
        );
        finished.load(Ordering::SeqCst) && before == self.counts()
    }

    fn ingress(&self) -> Ingress {
        self.pipeline.as_ref().unwrap().ingress()
    }

    fn health(&self) -> Arc<AtomicBool> {
        Arc::clone(
            &self
                .tracker
                .health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) fn foreign_error(&self) {
        self.ingress().send(
            &Arc::new(AtomicBool::new(false)),
            Err(notify::Error::generic("测试未发布实例错误")),
        );
    }

    pub(crate) fn foreign_paths(&self, paths: &[&str]) {
        let mut event = notify::Event::new(notify::EventKind::Any);
        event.paths = paths.iter().map(|path| self.root.join(path)).collect();
        self.ingress()
            .send(&Arc::new(AtomicBool::new(true)), Ok(event));
    }

    pub(crate) fn interleave_callbacks(&self) {
        let first = self.ingress();
        let second = self.ingress();
        let (allocated, allocation) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let (attempted, attempt) = mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                first.send_paused(&Arc::new(AtomicBool::new(false)), allocated, resumed)
            });
            allocation
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            scope.spawn(move || {
                // 首路已取得编号但尚未 send，次路必须确认同一个入口锁仍被持有。
                assert!(second.allocation_is_locked());
                attempted.send(()).unwrap();
                second.send(
                    &Arc::new(AtomicBool::new(false)),
                    Ok(notify::Event::new(notify::EventKind::Any)),
                );
            });
            attempt
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            resume.send(()).unwrap();
        });
    }

    pub(crate) fn concurrent_callbacks(&self, rounds: usize) {
        let first = self.ingress();
        let second = self.ingress();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let root = self.root.clone();
        std::thread::scope(|scope| {
            for ingress in [first, second] {
                let barrier = Arc::clone(&barrier);
                let root = root.clone();
                scope.spawn(move || {
                    let health = Arc::new(AtomicBool::new(false));
                    for _ in 0..rounds {
                        barrier.wait();
                        ingress.send(
                            &health,
                            Ok(notify::Event::new(notify::EventKind::Any)
                                .add_path(root.join("Cargo.toml"))),
                        );
                    }
                });
            }
        });
    }
}

pub(crate) fn completion(watcher: &SourceWatcher) -> Arc<AtomicBool> {
    watcher.pipeline.as_ref().unwrap().finished()
}

pub(crate) fn callback_during_registration_gate(
    watcher: &mut SourceWatcher,
    inputs: super::WatchInputRoots,
) -> crate::Result<bool> {
    let ingress = watcher.pipeline.as_ref().unwrap().ingress();
    let tracker = Arc::clone(&watcher.revision);
    // 同一线程持 gate 模拟后端确认前同步回调，随后仍通过真实注册接口重绑。
    {
        let _gate = tracker.lock_event_order();
        let health = Arc::clone(
            &tracker
                .health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        ingress.send(&health, Ok(notify::Event::new(notify::EventKind::Any)));
        if tracker.fence_under_gate(tracker.current_revision()) {
            return Ok(false);
        }
    }
    watcher.update_inputs(inputs)?;
    watcher.pipeline.as_ref().unwrap().barrier();
    Ok(true)
}

pub(crate) fn failed_replacement(
    watcher: &mut SourceWatcher,
    inputs: super::WatchInputRoots,
) -> crate::Result<()> {
    watcher.replace_inputs_with(
        inputs,
        super::subscriptions::Subscriptions::fail_during_prepare,
    )
}

pub(crate) fn registration_paths(watcher: &SourceWatcher) -> Vec<(PathBuf, bool)> {
    watcher
        .subscriptions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .unwrap()
        .registrations()
        .iter()
        .map(|(path, mode)| (path.clone(), *mode == notify::RecursiveMode::Recursive))
        .collect()
}

pub(crate) fn flush(watcher: &SourceWatcher) {
    watcher.pipeline.as_ref().unwrap().barrier();
}

pub(crate) fn replace_tools_parent_with_file(watcher: &SourceWatcher) -> bool {
    {
        let _gate = watcher.revision.lock_event_order();
        let root = watcher
            .mapping
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .root
            .clone();
        let parent = root.join(".local-tests");
        std::fs::rename(&parent, root.join(".previous-local-tests")).unwrap();
        std::fs::write(&parent, "must-not-echo-tools-parent-content").unwrap();
        let health = Arc::clone(
            &watcher
                .revision
                .health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        // gate 内完成替换并明确入队，不依赖原生后端是否合并目录事件。
        watcher.pipeline.as_ref().unwrap().ingress().send(
            &health,
            Ok(notify::Event::new(notify::EventKind::Any).add_path(parent)),
        );
    }
    flush(watcher);
    watcher.revision.failed.load(Ordering::SeqCst)
}

pub(crate) fn fail_current(watcher: &SourceWatcher) {
    let health = Arc::clone(
        &watcher
            .revision
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    watcher
        .pipeline
        .as_ref()
        .unwrap()
        .ingress()
        .send(&health, Err(notify::Error::generic("测试当前实例错误")));
}

pub(crate) fn new_fails_before_publish(
    watcher: &mut SourceWatcher,
    inputs: super::WatchInputRoots,
) -> crate::Result<()> {
    watcher.replace_inputs_with(inputs, |mapping, ingress| {
        let prepared =
            super::subscriptions::Subscriptions::new(&mapping.root, mapping, ingress.clone())?;
        ingress.send(
            &prepared.health(),
            Err(notify::Error::generic("测试新实例发布前错误")),
        );
        Ok(prepared)
    })
}

pub(crate) fn old_fails_before_publish(
    watcher: &mut SourceWatcher,
    inputs: super::WatchInputRoots,
) -> crate::Result<()> {
    let old_health = Arc::clone(
        &watcher
            .revision
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    watcher.replace_inputs_with(inputs, |mapping, ingress| {
        let prepared =
            super::subscriptions::Subscriptions::new(&mapping.root, mapping, ingress.clone())?;
        ingress.send(
            &old_health,
            Err(notify::Error::generic("测试旧实例在准备期间错误")),
        );
        Ok(prepared)
    })
}
