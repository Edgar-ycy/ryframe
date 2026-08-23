use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

/// 数据保留用例允许清理的固定资源。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RetentionResource {
    BackgroundJobs,
    OutboxEvents,
    ScheduleExecutions,
    ExportJobs,
    OperationLogs,
    LoginLogs,
    UserImports,
    ServiceAccessAudits,
    RetentionRuns,
}

impl RetentionResource {
    pub const ALL: [Self; 9] = [
        Self::BackgroundJobs,
        Self::OutboxEvents,
        Self::ScheduleExecutions,
        Self::ExportJobs,
        Self::OperationLogs,
        Self::LoginLogs,
        Self::UserImports,
        Self::ServiceAccessAudits,
        Self::RetentionRuns,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::BackgroundJobs => "background_jobs",
            Self::OutboxEvents => "outbox_events",
            Self::ScheduleExecutions => "schedule_executions",
            Self::ExportJobs => "export_jobs",
            Self::OperationLogs => "operation_logs",
            Self::LoginLogs => "login_logs",
            Self::UserImports => "user_imports",
            Self::ServiceAccessAudits => "service_access_audits",
            Self::RetentionRuns => "retention_runs",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RetentionCutoff {
    pub resource: RetentionResource,
    pub before: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetentionCleanupResult {
    pub deleted: u64,
    pub remaining: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub struct ExpiredImportArtifact {
    pub tenant_id: String,
    pub file_id: i64,
}

#[async_trait::async_trait]
pub trait RetentionCleanupPersistencePort: Send + Sync {
    async fn preview<'a>(
        &'a self,
        cutoffs: &'a [RetentionCutoff],
        current_run_id: Option<i64>,
    ) -> ryframe_kernel::AppResult<BTreeMap<String, u64>>;

    async fn cleanup_resource(
        &self,
        cutoff: RetentionCutoff,
        batch_size: usize,
        maximum: usize,
        current_run_id: Option<i64>,
    ) -> ryframe_kernel::AppResult<RetentionCleanupResult>;

    async fn count_expired_import_artifacts(
        &self,
        before: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<u64>;

    async fn list_expired_import_artifacts(
        &self,
        before: DateTime<Utc>,
        after_id: Option<i64>,
        limit: usize,
    ) -> ryframe_kernel::AppResult<Vec<ExpiredImportArtifact>>;
}
