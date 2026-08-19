mod backoff;
mod handlers;
mod metrics;
mod outbox;
mod queue;
mod schedule;
mod schedule_compat;
mod schedule_metrics;
mod schedule_targets;
mod wakeup;
mod worker;

pub use handlers::{
    ExportCleanupJobHandler, ExportJobHandler, MessageDispatchJobHandler,
    MessageRetentionJobHandler,
};
pub use metrics::{CallbackJobMetricsObserver, JobMetricsObserver};
pub use outbox::{OutboxRunResult, OutboxWorker};
pub use queue::{BackgroundJobListParams, BackgroundJobQueueStats, BackgroundJobVo, JobQueue};
pub use schedule::{
    CreateJobSchedule, JobScheduleExecutionListParams, JobScheduleExecutionVo,
    JobScheduleListParams, JobScheduleOccurrence, JobSchedulePreview, JobScheduleService,
    JobScheduleVo, UpdateJobSchedule,
};
#[allow(deprecated)]
pub use schedule_compat::spawn_message_retention_scheduler;
pub use schedule_metrics::{CallbackScheduleMetricsObserver, ScheduleMetricsObserver};
pub use schedule_targets::{
    ScheduledJobContext, ScheduledJobTarget, ScheduledJobTargetDescriptor,
    ScheduledJobTargetRegistry, ScheduledJobTargetScope,
};
pub use worker::{JobHandler, JobRunResult, JobWorker};

/// 消息发布 Outbox 事件的稳定类型标识。
pub const MESSAGE_PUBLISHED_OUTBOX_EVENT_TYPE: &str = "system.message.published";
