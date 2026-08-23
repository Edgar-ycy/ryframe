use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use futures_util::stream;
use ryframe_application::jobs::{
    JOB_WAKEUP_REDIS_CHANNEL, JobWakeupStream, JobWakeupTransport, QueueWakeup, WakeupQueue,
};

#[derive(Default)]
struct RecordingTransport {
    published: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl JobWakeupTransport for RecordingTransport {
    async fn publish(&self, channel: &str, payload: &str) -> Result<(), String> {
        self.published
            .lock()
            .expect("记录锁不应中毒")
            .push((channel.to_owned(), payload.to_owned()));
        Ok(())
    }

    async fn subscribe(&self, _channel: &str) -> Result<JobWakeupStream, String> {
        Ok(Box::pin(stream::empty()) as JobWakeupStream)
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
            r#"{"v":1,"queue":"background_job"}"#.to_owned(),
        )]
    );
}
