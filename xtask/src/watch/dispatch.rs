use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use super::{SourceRevisionTracker, WatchEvent, inputs::Mapping, subscriptions::Subscriptions};
use crate::Result;

pub(super) type Backend = Arc<Mutex<Option<Subscriptions>>>;

enum RawMessage {
    Event {
        sequence: u64,
        health: Arc<AtomicBool>,
        event: notify::Result<notify::Event>,
    },
    Fatal,
    Stop,
    #[cfg(test)]
    Barrier(Sender<()>),
}

#[derive(Clone)]
pub(super) struct Ingress {
    sender: Sender<RawMessage>,
    tracker: Arc<SourceRevisionTracker>,
    closing: Arc<AtomicBool>,
    raw_order: Arc<Mutex<()>>,
}

impl Ingress {
    pub(super) fn send(&self, health: &Arc<AtomicBool>, event: notify::Result<notify::Event>) {
        self.enqueue(health, event, || {});
    }

    fn enqueue(
        &self,
        health: &Arc<AtomicBool>,
        event: notify::Result<notify::Event>,
        after_allocation: impl FnOnce(),
    ) {
        // 该短锁只串行化 raw 编号和入队，永不获取 event_order 或等待注册确认。
        let _order = self
            .raw_order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        let sequence = match self.tracker.raw_arrivals.try_update(
            Ordering::SeqCst,
            Ordering::SeqCst,
            |value| value.checked_add(1),
        ) {
            Ok(previous) => previous + 1,
            Err(_) => {
                self.tracker.failed.store(true, Ordering::SeqCst);
                let _ = self.sender.send(RawMessage::Fatal);
                return;
            }
        };
        after_allocation();
        let event = match event {
            Ok(event) if event.need_rescan() => Err(notify::Error::generic("监听缓冲区丢失事件")),
            other => other,
        };
        if event.is_err() {
            health.store(true, Ordering::SeqCst);
        }
        if self
            .sender
            .send(RawMessage::Event {
                sequence,
                health: Arc::clone(health),
                event,
            })
            .is_err()
        {
            self.tracker.failed.store(true, Ordering::SeqCst);
        }
    }

    #[cfg(test)]
    pub(super) fn send_paused(
        &self,
        health: &Arc<AtomicBool>,
        allocated: Sender<()>,
        resume: Receiver<()>,
    ) {
        self.enqueue(
            health,
            Ok(notify::Event::new(notify::EventKind::Any)),
            || {
                allocated.send(()).unwrap();
                resume
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            },
        );
    }

    #[cfg(test)]
    pub(super) fn allocation_is_locked(&self) -> bool {
        matches!(
            self.raw_order.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        )
    }
}

pub(super) struct Pipeline {
    ingress: Option<Ingress>,
    thread: Option<JoinHandle<()>>,
    #[cfg(test)]
    finished: Arc<AtomicBool>,
}

impl Pipeline {
    pub(super) fn start(
        tracker: Arc<SourceRevisionTracker>,
        mapping: Arc<Mutex<Mapping>>,
        backend: Backend,
        logical: Sender<WatchEvent>,
    ) -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let ingress = Ingress {
            sender,
            tracker: Arc::clone(&tracker),
            closing: Arc::new(AtomicBool::new(false)),
            raw_order: Arc::new(Mutex::new(())),
        };
        #[cfg(test)]
        let finished = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let thread_finished = Arc::clone(&finished);
        let thread = thread::Builder::new().name("ryframe-dev-watch-dispatch".into()).spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(receiver, &tracker, &mapping, &backend, &logical)));
            if outcome.is_err() {
                fail(&tracker, &logical, "源码监听分发线程异常退出；请停止监督器并检查监听输入后重新启动".into());
            }
            #[cfg(test)]
            thread_finished.store(true, Ordering::SeqCst);
        }).map_err(|error| format!("启动源码监听分发线程失败：error.kind={:?}，raw_os_error={:?}；请检查线程资源后重新启动", error.kind(), error.raw_os_error()))?;
        Ok(Self {
            ingress: Some(ingress),
            thread: Some(thread),
            #[cfg(test)]
            finished,
        })
    }

    pub(super) fn ingress(&self) -> Ingress {
        self.ingress
            .as_ref()
            .expect("存活的分发器持有 raw 入口")
            .clone()
    }

    #[cfg(test)]
    pub(super) fn barrier(&self) {
        let (sender, receiver) = mpsc::channel();
        self.ingress()
            .sender
            .send(RawMessage::Barrier(sender))
            .unwrap();
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }

    #[cfg(test)]
    pub(super) fn finished(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.finished)
    }

    #[cfg(test)]
    pub(super) fn inject_sequence(&self, sequence: u64) {
        self.ingress()
            .sender
            .send(RawMessage::Event {
                sequence,
                health: Arc::new(AtomicBool::new(false)),
                event: Ok(notify::Event::new(notify::EventKind::Any)),
            })
            .unwrap();
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        if let Some(ingress) = self.ingress.take() {
            {
                let _order = ingress
                    .raw_order
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ingress.closing.store(true, Ordering::SeqCst);
                let _ = ingress.sender.send(RawMessage::Stop);
            }
            drop(ingress);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    receiver: Receiver<RawMessage>,
    tracker: &SourceRevisionTracker,
    mapping: &Mutex<Mapping>,
    backend: &Backend,
    logical: &Sender<WatchEvent>,
) {
    while let Ok(message) = receiver.recv() {
        let (sequence, health, event) = match message {
            RawMessage::Stop => return,
            RawMessage::Fatal => {
                fail(
                    tracker,
                    logical,
                    "源码 raw 到达编号耗尽；请停止监督器并检查代次后重新启动".into(),
                );
                continue;
            }
            #[cfg(test)]
            RawMessage::Barrier(sender) => {
                let _ = sender.send(());
                continue;
            }
            RawMessage::Event {
                sequence,
                health,
                event,
            } => (sequence, health, event),
        };
        let _gate = tracker.lock_event_order();
        if tracker.applied_raw.load(Ordering::SeqCst).checked_add(1) != Some(sequence) {
            fail(
                tracker,
                logical,
                "源码 raw 事件编号不连续；拒绝跳过缺口，请停止监督器并重新建立监听".into(),
            );
            continue;
        }
        match event {
            Ok(event) => process_event(event, tracker, mapping, backend, logical),
            Err(error) => {
                let current = tracker
                    .health
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if Arc::ptr_eq(&health, &current) {
                    let _ = logical.send(WatchEvent::Failed(notify_error(
                        "处理当前实例原生源码事件",
                        &error,
                    )));
                }
            }
        }
        // 只有相关/无关/错误事件完整处理后才确认该连续编号。
        tracker.applied_raw.store(sequence, Ordering::SeqCst);
    }
}

fn process_event(
    event: notify::Event,
    tracker: &SourceRevisionTracker,
    mapping: &Mutex<Mapping>,
    backend: &Backend,
    logical: &Sender<WatchEvent>,
) {
    // 读取/遍历只确认 raw 完成；注册递归根自身的目录读取不代表源码发生变化。
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return;
    }
    let mapping = mapping
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut paths = mapping.paths(&event.paths);
    let mut backend = backend
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(backend) = backend.as_mut() {
        match backend.observe_tools_parent(&mapping, &event.paths, tracker) {
            Ok(discovered) => paths.extend(discovered),
            Err(error) => {
                fail(tracker, logical, error.to_string());
                return;
            }
        }
    }
    if !paths.is_empty() {
        publish(tracker, logical, paths);
    }
}

pub(super) fn publish(
    tracker: &SourceRevisionTracker,
    sender: &Sender<WatchEvent>,
    paths: BTreeSet<String>,
) {
    if let Some(revision) = tracker.next_revision() {
        if sender
            .send(WatchEvent::BackendChanged { revision, paths })
            .is_err()
        {
            tracker.failed.store(true, Ordering::SeqCst);
        }
    } else {
        fail(
            tracker,
            sender,
            "源码逻辑代次耗尽；请停止监督器并检查保存代次后重新启动".into(),
        );
    }
}

pub(super) fn fail(tracker: &SourceRevisionTracker, sender: &Sender<WatchEvent>, message: String) {
    tracker.failed.store(true, Ordering::SeqCst);
    let _ = sender.send(WatchEvent::Failed(message));
}

pub(super) fn notify_error(stage: &str, error: &notify::Error) -> String {
    let category = match &error.kind {
        notify::ErrorKind::Io(error) => format!(
            "I/O error.kind={:?}，raw_os_error={:?}",
            error.kind(),
            error.raw_os_error()
        ),
        notify::ErrorKind::PathNotFound => "路径不存在".into(),
        notify::ErrorKind::WatchNotFound => "监听登记不存在".into(),
        notify::ErrorKind::MaxFilesWatch => "原生监听数量达到系统上限".into(),
        notify::ErrorKind::InvalidConfig(_) => "原生监听配置无效".into(),
        notify::ErrorKind::Generic(_) => "原生监听后端报告错误".into(),
    };
    let paths = error
        .paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("、");
    format!(
        "{stage}失败：路径 [{paths}]，类别 {category}；请检查普通目录、访问权限和系统监听资源，停止监督器后重新启动"
    )
}
