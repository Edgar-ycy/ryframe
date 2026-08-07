mod handlers;
mod metrics;
mod outbox;
mod queue;
mod wakeup;
mod worker;

pub use handlers::{
    ExportCleanupJobHandler, ExportJobHandler, MessageDispatchJobHandler,
    MessageRetentionJobHandler,
};
pub use metrics::{CallbackJobMetricsObserver, JobMetricsObserver};
pub use outbox::{OutboxRunResult, OutboxWorker};
pub use queue::{
    BackgroundJobListParams, BackgroundJobQueueStats, BackgroundJobVo, JobQueue,
    spawn_message_retention_scheduler,
};
pub use worker::{JobHandler, JobRunResult, JobWorker};

/// 消息发布 Outbox 事件的稳定类型标识。
pub const MESSAGE_PUBLISHED_OUTBOX_EVENT_TYPE: &str = "system.message.published";
