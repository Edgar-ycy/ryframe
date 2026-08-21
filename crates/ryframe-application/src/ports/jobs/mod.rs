//! 后台任务、调度和 Outbox 执行所需的持久化端口。

mod outbox;
mod queue;
mod schedule;
mod tenant_scope;

pub use outbox::{ClaimedOutboxEvent, OutboxFailureOutcome, OutboxPersistencePort};
pub use queue::{
    BackgroundJobPersistencePort, BackgroundJobReadFilter, BackgroundJobRecord,
    BackgroundJobStatsRecord, BackgroundJobTransaction, BackgroundJobTypeStats, ClaimedJobRecord,
    FailJobCommand, JobFailureOutcome, RecoveredJobLeases, TenantConfigJobKind,
};
pub use schedule::{
    JobScheduleExecutionReadFilter, JobScheduleExecutionRecord, JobSchedulePersistencePort,
    JobScheduleReadFilter, JobScheduleReadPort, JobScheduleRecord, JobScheduleTransaction,
    NewJobScheduleExecution,
};
pub use tenant_scope::ExecutionTenantScope;
