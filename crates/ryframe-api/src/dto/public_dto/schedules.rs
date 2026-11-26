use ryframe_application::ports::utc_timestamp;
use ryframe_application::{
    JobScheduleExecutionVo as ServiceExecutionVo, JobScheduleOccurrence as ServiceOccurrence,
    JobSchedulePreview as ServicePreview, JobScheduleVo as ServiceScheduleVo,
    ScheduledJobTargetDescriptor as ServiceTargetDescriptor, ScheduledJobTargetScope,
};
use serde::Serialize;
use time::OffsetDateTime;
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
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub next_run_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_run_at: Option<OffsetDateTime>,
    pub version: i64,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: OffsetDateTime,
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
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub scheduled_for: OffsetDateTime,
    pub outcome: String,
    pub background_job_id: Option<String>,
    pub background_job_status: Option<String>,
    pub detail: Option<String>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
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
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub utc: OffsetDateTime,
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
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub calculated_at: OffsetDateTime,
    pub timezone: String,
    pub occurrences: Vec<JobScheduleOccurrence>,
}

impl From<ServicePreview> for JobSchedulePreview {
    fn from(value: ServicePreview) -> Self {
        Self {
            calculated_at: value.calculated_at,
            timezone: value.timezone,
            occurrences: value
                .occurrences
                .into_iter()
                .map(JobScheduleOccurrence::from)
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use time::macros::datetime;
    use utoipa::OpenApi;

    #[test]
    fn schedule_response_preserves_utc_precision_and_snapshot_schema() {
        let mut schedule = ServiceScheduleVo {
            id: "42".into(),
            name: "调度样例".into(),
            handler_key: "fixture.job".into(),
            cron_expression: "0 0 0 * * * *".into(),
            timezone: "Asia/Shanghai".into(),
            enabled: true,
            misfire_policy: "fire_once".into(),
            concurrency_policy: "forbid".into(),
            max_runtime_seconds: 300,
            next_run_at: Some(datetime!(1970-01-02 00:00:00 UTC)),
            last_run_at: Some(datetime!(1970-01-01 07:59:58.120 +08:00)),
            version: 7,
            created_at: datetime!(1969-12-31 23:59:57.123400 UTC),
            updated_at: datetime!(1969-12-31 23:59:59.123456700 UTC),
        };
        let encoded = serde_json::to_string(&JobScheduleVo::from(schedule.clone())).unwrap();
        assert_eq!(encoded, serde_json::to_string(&schedule).unwrap());
        let body: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(body["next_run_at"], "1970-01-02T00:00:00Z");
        assert_eq!(body["last_run_at"], "1969-12-31T23:59:58.120Z");
        assert_eq!(body["created_at"], "1969-12-31T23:59:57.123400Z");
        assert_eq!(body["updated_at"], "1969-12-31T23:59:59.123456700Z");

        schedule.next_run_at = None;
        schedule.last_run_at = None;
        let body = serde_json::to_value(JobScheduleVo::from(schedule)).unwrap();
        assert_eq!(body.get("next_run_at"), Some(&Value::Null));
        assert_eq!(body.get("last_run_at"), Some(&Value::Null));

        let snapshot: Value =
            serde_json::from_str(include_str!("../../../../../openapi/openapi.json")).unwrap();
        let generated = serde_json::to_value(crate::openapi::ApiDoc::openapi()).unwrap();
        let schema_path = "/components/schemas/JobScheduleVo";
        assert_eq!(
            generated.pointer(schema_path),
            Some(
                snapshot
                    .pointer(schema_path)
                    .expect("快照包含定时任务响应契约")
            )
        );
    }
}
