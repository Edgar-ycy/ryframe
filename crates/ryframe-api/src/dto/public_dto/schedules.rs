use chrono::{DateTime, Utc};
use ryframe_service::{
    JobScheduleExecutionVo as ServiceExecutionVo, JobScheduleOccurrence as ServiceOccurrence,
    JobSchedulePreview as ServicePreview, JobScheduleVo as ServiceScheduleVo,
    ScheduledJobTargetDescriptor as ServiceTargetDescriptor, ScheduledJobTargetScope,
};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ScheduleTargetVo {
    pub handler_key: String,
    pub display_name: String,
    pub scope: String,
    pub job_type: String,
    pub available: bool,
}

impl From<ServiceTargetDescriptor> for ScheduleTargetVo {
    fn from(value: ServiceTargetDescriptor) -> Self {
        Self {
            handler_key: value.handler_key,
            display_name: value.display_name,
            scope: match value.scope {
                ScheduledJobTargetScope::Tenant => "tenant",
                ScheduledJobTargetScope::System => "system",
            }
            .to_owned(),
            job_type: value.job_type,
            available: value.available,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobScheduleVo {
    pub id: String,
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub misfire_policy: String,
    pub concurrency_policy: String,
    pub max_runtime_seconds: i32,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ServiceScheduleVo> for JobScheduleVo {
    fn from(value: ServiceScheduleVo) -> Self {
        Self {
            id: value.id,
            name: value.name,
            handler_key: value.handler_key,
            cron_expression: value.cron_expression,
            timezone: value.timezone,
            enabled: value.enabled,
            misfire_policy: value.misfire_policy,
            concurrency_policy: value.concurrency_policy,
            max_runtime_seconds: value.max_runtime_seconds,
            next_run_at: value.next_run_at,
            last_run_at: value.last_run_at,
            version: value.version,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobScheduleExecutionVo {
    pub id: String,
    pub schedule_id: String,
    pub schedule_name: String,
    pub handler_key: String,
    pub trigger_kind: String,
    pub scheduled_for: DateTime<Utc>,
    pub outcome: String,
    pub background_job_id: Option<String>,
    pub background_job_status: Option<String>,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceExecutionVo> for JobScheduleExecutionVo {
    fn from(value: ServiceExecutionVo) -> Self {
        Self {
            id: value.id,
            schedule_id: value.schedule_id,
            schedule_name: value.schedule_name,
            handler_key: value.handler_key,
            trigger_kind: value.trigger_kind,
            scheduled_for: value.scheduled_for,
            outcome: value.outcome,
            background_job_id: value.background_job_id,
            background_job_status: value.background_job_status,
            detail: value.detail,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobScheduleOccurrence {
    pub utc: DateTime<Utc>,
    pub schedule_time: String,
}

impl From<ServiceOccurrence> for JobScheduleOccurrence {
    fn from(value: ServiceOccurrence) -> Self {
        Self {
            utc: value.utc,
            schedule_time: value.schedule_time,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobSchedulePreview {
    pub timezone: String,
    pub occurrences: Vec<JobScheduleOccurrence>,
}

impl From<ServicePreview> for JobSchedulePreview {
    fn from(value: ServicePreview) -> Self {
        Self {
            timezone: value.timezone,
            occurrences: value
                .occurrences
                .into_iter()
                .map(JobScheduleOccurrence::from)
                .collect(),
        }
    }
}
