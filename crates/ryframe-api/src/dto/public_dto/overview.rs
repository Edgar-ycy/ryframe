use chrono::{DateTime, Utc};
use ryframe_adapters::monitor::ServerInfo;
use ryframe_application::system as service;
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct MonitorOverviewVo {
    pub calculated_at: DateTime<Utc>,
    pub dependencies: MonitorOverviewDependenciesVo,
    pub system: MonitorOverviewSystemVo,
    pub database_pool: MonitorOverviewDatabasePoolVo,
    pub jobs: MonitorOverviewJobsVo,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MonitorOverviewDependenciesVo {
    pub database: MonitorOverviewDependencyVo,
    pub redis: MonitorOverviewDependencyVo,
    pub object_storage: MonitorOverviewDependencyVo,
    pub messaging: MonitorOverviewDependencyVo,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MonitorOverviewDependencyVo {
    pub status: String,
    pub configured: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MonitorOverviewSystemVo {
    pub os: String,
    pub hostname: String,
    pub cpu_cores: usize,
    pub cpu_usage: f32,
    pub total_memory_gb: f64,
    pub used_memory_gb: f64,
    pub memory_usage: f32,
    pub process_id: String,
    pub uptime_seconds: u64,
    pub process_status: String,
}

impl From<ServerInfo> for MonitorOverviewSystemVo {
    fn from(value: ServerInfo) -> Self {
        Self {
            os: value.os.to_string(),
            hostname: value.hostname.to_string(),
            cpu_cores: value.cpu_cores,
            cpu_usage: value.cpu_usage,
            total_memory_gb: value.total_memory,
            used_memory_gb: value.used_memory,
            memory_usage: value.memory_usage,
            process_id: value.pid.to_string(),
            uptime_seconds: value.uptime,
            process_status: "up".to_owned(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MonitorOverviewDatabasePoolVo {
    pub status: String,
    pub active_connections: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MonitorOverviewJobsVo {
    pub mode: String,
    pub scheduler_enabled: bool,
    pub total: u64,
    pub ready: u64,
    pub pending: u64,
    pub running: u64,
    pub succeeded: u64,
    pub dead: u64,
    pub enabled_schedules: u64,
    pub schedule_lag_seconds: f64,
}

impl From<service::OverviewCoreSnapshot> for MonitorOverviewJobsVo {
    fn from(value: service::OverviewCoreSnapshot) -> Self {
        Self {
            mode: value.jobs_mode,
            scheduler_enabled: value.scheduler_enabled,
            total: value.background_jobs.total,
            ready: value.background_jobs.ready,
            pending: value.background_jobs.pending,
            running: value.background_jobs.running,
            succeeded: value.background_jobs.succeeded,
            dead: value.background_jobs.dead,
            enabled_schedules: value.enabled_schedules,
            schedule_lag_seconds: value.schedule_lag_seconds,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MonitorOverviewTrendsVo {
    pub calculated_at: DateTime<Utc>,
    pub range: String,
    pub bucket_seconds: u32,
    pub buckets: Vec<MonitorOverviewTrendBucketVo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MonitorOverviewTrendBucketVo {
    pub started_at: DateTime<Utc>,
    pub background_jobs_created: u64,
    pub schedule_enqueued: u64,
    pub schedule_skipped_misfire: u64,
    pub schedule_skipped_concurrency: u64,
    pub schedule_target_unavailable: u64,
    pub schedule_invalid_configuration: u64,
    pub login_success: u64,
    pub login_failure: u64,
    pub operation_success: u64,
    pub operation_failure: u64,
}

impl From<service::OverviewTrends> for MonitorOverviewTrendsVo {
    fn from(value: service::OverviewTrends) -> Self {
        Self {
            calculated_at: value.calculated_at,
            range: value.range,
            bucket_seconds: value.bucket_seconds,
            buckets: value.buckets.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<service::OverviewTrendBucket> for MonitorOverviewTrendBucketVo {
    fn from(value: service::OverviewTrendBucket) -> Self {
        Self {
            started_at: value.started_at,
            background_jobs_created: value.background_jobs_created,
            schedule_enqueued: value.schedule_enqueued,
            schedule_skipped_misfire: value.schedule_skipped_misfire,
            schedule_skipped_concurrency: value.schedule_skipped_concurrency,
            schedule_target_unavailable: value.schedule_target_unavailable,
            schedule_invalid_configuration: value.schedule_invalid_configuration,
            login_success: value.login_success,
            login_failure: value.login_failure,
            operation_success: value.operation_success,
            operation_failure: value.operation_failure,
        }
    }
}
