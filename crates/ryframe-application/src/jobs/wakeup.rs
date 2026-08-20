use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use tokio::{sync::watch, task::JoinHandle};

use super::metrics::JobMetricsObserver;

/// 后台任务和 Outbox 共用的 Redis 唤醒频道。
pub const JOB_WAKEUP_REDIS_CHANNEL: &str = "ryframe:jobs:wakeup";

pub type JobWakeupStream = Pin<Box<dyn Stream<Item = Result<String, String>> + Send>>;
pub type JobWakeupFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// 后台任务跨进程唤醒传输端口。
pub trait JobWakeupTransport: Send + Sync {
    fn publish<'a>(&'a self, channel: &'a str, payload: &'a str) -> JobWakeupFuture<'a, ()>;

    fn subscribe<'a>(&'a self, channel: &'a str) -> JobWakeupFuture<'a, JobWakeupStream>;
}

const WAKEUP_PROTOCOL_VERSION: u8 = 1;
const BACKGROUND_JOB_WAKEUP_PAYLOAD: &str = r#"{"v":1,"queue":"background_job"}"#;
const OUTBOX_WAKEUP_PAYLOAD: &str = r#"{"v":1,"queue":"outbox"}"#;

/// 仅用于内部唤醒协议的两个持久化队列。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WakeupQueue {
    BackgroundJob,
    Outbox,
}

impl WakeupQueue {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::BackgroundJob => "background_job",
            Self::Outbox => "outbox",
        }
    }

    const fn payload(self) -> &'static str {
        match self {
            Self::BackgroundJob => BACKGROUND_JOB_WAKEUP_PAYLOAD,
            Self::Outbox => OUTBOX_WAKEUP_PAYLOAD,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "background_job" => Some(Self::BackgroundJob),
            "outbox" => Some(Self::Outbox),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct RedisWakeupPayload {
    v: u8,
    queue: String,
}

/// 本进程队列等待与可选 Redis Pub/Sub 提示的协调器。
///
/// 唤醒信号并不承载业务事实。任何信号丢失、重复、解析失败或 Redis 不可用时，
/// 消费循环都会继续通过 MySQL 轮询领取任务。
#[derive(Clone)]
pub(super) struct QueueWakeup {
    inner: Arc<QueueWakeupInner>,
}

struct QueueWakeupInner {
    transport: Option<Arc<dyn JobWakeupTransport>>,
    background_job: watch::Sender<u64>,
    outbox: watch::Sender<u64>,
    listener_started: AtomicBool,
    metrics_observer: Arc<RwLock<Option<Arc<dyn JobMetricsObserver>>>>,
}

impl QueueWakeup {
    pub(super) fn new(
        transport: Option<Arc<dyn JobWakeupTransport>>,
        metrics_observer: Arc<RwLock<Option<Arc<dyn JobMetricsObserver>>>>,
    ) -> Self {
        let (background_job, _) = watch::channel(0_u64);
        let (outbox, _) = watch::channel(0_u64);
        Self {
            inner: Arc::new(QueueWakeupInner {
                transport,
                background_job,
                outbox,
                listener_started: AtomicBool::new(false),
                metrics_observer,
            }),
        }
    }

    pub(super) fn subscribe(&self, queue: WakeupQueue) -> watch::Receiver<u64> {
        match queue {
            WakeupQueue::BackgroundJob => self.inner.background_job.subscribe(),
            WakeupQueue::Outbox => self.inner.outbox.subscribe(),
        }
    }

    /// 先通知本进程等待者，再尝试发布跨进程提示。Redis 失败只会被观测，不会传回调用方。
    pub(super) async fn notify(&self, queue: WakeupQueue) {
        self.notify_local(queue);
        self.record_wakeup(queue, "local", "success");

        let Some(transport) = self.inner.transport.as_ref() else {
            self.record_wakeup(queue, "redis", "bypass");
            return;
        };
        match transport
            .publish(JOB_WAKEUP_REDIS_CHANNEL, queue.payload())
            .await
        {
            Ok(_) => self.record_wakeup(queue, "redis", "success"),
            Err(error) => {
                self.record_wakeup(queue, "redis", "error");
                tracing::debug!(
                    %error,
                    queue = queue.as_str(),
                    channel = JOB_WAKEUP_REDIS_CHANNEL,
                    "后台任务 Redis 唤醒提示发送失败，将继续依赖数据库轮询"
                );
            }
        }
    }

    /// 启动当前进程唯一的 Redis 订阅循环；没有 Redis 时只保留本地唤醒。
    pub(super) fn spawn_redis_listener(
        &self,
        shutdown: watch::Receiver<bool>,
    ) -> Option<JoinHandle<()>> {
        let Some(transport) = self.inner.transport.clone() else {
            self.set_listener_up(false);
            return None;
        };
        if self.inner.listener_started.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.set_listener_up(false);
        let wakeup = self.clone();
        Some(tokio::spawn(async move {
            wakeup.transport_listener(transport, shutdown).await;
        }))
    }

    fn notify_local(&self, queue: WakeupQueue) {
        let sender = match queue {
            WakeupQueue::BackgroundJob => &self.inner.background_job,
            WakeupQueue::Outbox => &self.inner.outbox,
        };
        sender.send_modify(|revision| *revision = revision.wrapping_add(1));
    }

    async fn transport_listener(
        &self,
        transport: Arc<dyn JobWakeupTransport>,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let mut retry_seconds = 1_u64;
        let mut degraded = false;
        loop {
            if *shutdown.borrow() {
                break;
            }
            match transport.subscribe(JOB_WAKEUP_REDIS_CHANNEL).await {
                Ok(mut messages) => {
                    self.set_listener_up(true);
                    if degraded {
                        tracing::info!(
                            channel = JOB_WAKEUP_REDIS_CHANNEL,
                            "后台任务 Redis 唤醒订阅已恢复"
                        );
                    }
                    degraded = false;
                    retry_seconds = 1;
                    let interrupted = loop {
                        tokio::select! {
                            changed = shutdown.changed() => {
                                if changed.is_err() || *shutdown.borrow() {
                                    self.set_listener_up(false);
                                    return;
                                }
                            }
                            message = messages.next() => {
                                let Some(message) = message else {
                                    break true;
                                };
                                self.handle_transport_message(message);
                            }
                        }
                    };
                    self.set_listener_up(false);
                    if interrupted {
                        self.record_listener_failure(&mut degraded, None);
                    }
                }
                Err(error) => {
                    self.set_listener_up(false);
                    self.record_listener_failure(&mut degraded, Some(&error));
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(retry_seconds)) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
            retry_seconds = retry_seconds.saturating_mul(2).min(30);
        }
        self.set_listener_up(false);
    }

    fn handle_transport_message(&self, payload: Result<String, String>) {
        let payload = match payload {
            Ok(payload) => payload,
            Err(error) => {
                self.record_wakeup_protocol_error("decode_error");
                tracing::debug!(%error, "收到无法解析的后台任务 Redis 唤醒负载");
                return;
            }
        };
        let parsed: RedisWakeupPayload = match serde_json::from_str(&payload) {
            Ok(parsed) => parsed,
            Err(error) => {
                self.record_wakeup_protocol_error("invalid_payload");
                tracing::debug!(%error, "收到无效的后台任务 Redis 唤醒负载");
                return;
            }
        };
        if parsed.v != WAKEUP_PROTOCOL_VERSION {
            self.record_wakeup_protocol_error("unknown_version");
            tracing::debug!(version = parsed.v, "收到未知版本的后台任务 Redis 唤醒负载");
            return;
        }
        let Some(queue) = WakeupQueue::parse(&parsed.queue) else {
            self.record_wakeup_protocol_error("unknown_queue");
            tracing::debug!("收到未知队列的后台任务 Redis 唤醒负载");
            return;
        };
        self.notify_local(queue);
        self.record_wakeup(queue, "redis", "success");
    }

    fn record_listener_failure(&self, degraded: &mut bool, error: Option<&str>) {
        if *degraded {
            if let Some(error) = error {
                tracing::debug!(%error, channel = JOB_WAKEUP_REDIS_CHANNEL, "后台任务 Redis 唤醒订阅仍不可用");
            } else {
                tracing::debug!(
                    channel = JOB_WAKEUP_REDIS_CHANNEL,
                    "后台任务 Redis 唤醒订阅已中断"
                );
            }
            return;
        }
        if let Some(error) = error {
            tracing::warn!(%error, channel = JOB_WAKEUP_REDIS_CHANNEL, "后台任务 Redis 唤醒订阅失败，将退避重连");
        } else {
            tracing::warn!(
                channel = JOB_WAKEUP_REDIS_CHANNEL,
                "后台任务 Redis 唤醒订阅已中断，将退避重连"
            );
        }
        *degraded = true;
    }

    fn record_wakeup(&self, queue: WakeupQueue, transport: &'static str, result: &'static str) {
        if let Some(observer) = self.metrics_observer() {
            observer.record_wakeup(queue.as_str(), transport, result);
        }
    }

    fn record_wakeup_protocol_error(&self, result: &'static str) {
        if let Some(observer) = self.metrics_observer() {
            observer.record_wakeup_protocol_error(result);
        }
    }

    fn set_listener_up(&self, up: bool) {
        if let Some(observer) = self.metrics_observer() {
            observer.set_wakeup_listener_up(WakeupQueue::BackgroundJob.as_str(), up);
            observer.set_wakeup_listener_up(WakeupQueue::Outbox.as_str(), up);
        }
    }

    fn metrics_observer(&self) -> Option<Arc<dyn JobMetricsObserver>> {
        self.inner
            .metrics_observer
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures_util::stream;

    use super::*;

    #[derive(Default)]
    struct RecordingTransport {
        published: Mutex<Vec<(String, String)>>,
    }

    impl JobWakeupTransport for RecordingTransport {
        fn publish<'a>(&'a self, channel: &'a str, payload: &'a str) -> JobWakeupFuture<'a, ()> {
            Box::pin(async move {
                self.published
                    .lock()
                    .expect("记录锁不应中毒")
                    .push((channel.to_owned(), payload.to_owned()));
                Ok(())
            })
        }

        fn subscribe<'a>(&'a self, _channel: &'a str) -> JobWakeupFuture<'a, JobWakeupStream> {
            Box::pin(async { Ok(Box::pin(stream::empty()) as JobWakeupStream) })
        }
    }

    #[tokio::test]
    async fn notifies_local_waiter_and_transport() {
        let transport = Arc::new(RecordingTransport::default());
        let wakeup = QueueWakeup::new(
            Some(Arc::clone(&transport) as Arc<dyn JobWakeupTransport>),
            Arc::new(RwLock::new(None)),
        );
        let mut receiver = wakeup.subscribe(WakeupQueue::BackgroundJob);

        wakeup.notify(WakeupQueue::BackgroundJob).await;
        receiver.changed().await.expect("本地唤醒通道应保持有效");

        assert_eq!(*receiver.borrow(), 1);
        assert_eq!(
            transport
                .published
                .lock()
                .expect("记录锁不应中毒")
                .as_slice(),
            [(
                JOB_WAKEUP_REDIS_CHANNEL.to_owned(),
                BACKGROUND_JOB_WAKEUP_PAYLOAD.to_owned(),
            )]
        );
    }
}
