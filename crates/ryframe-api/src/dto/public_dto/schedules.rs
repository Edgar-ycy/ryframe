use ryframe_application::ports::utc_timestamp;
use ryframe_application::{
    JobScheduleExecutionVo as ServiceExecutionVo, JobScheduleOccurrence as ServiceOccurrence,
    JobSchedulePreview as ServicePreview, JobScheduleVo as ServiceScheduleVo,
    ScheduledJobTargetDescriptor as ServiceTargetDescriptor, ScheduledJobTargetScope,
};
use ryframe_kernel::{AppError, AppResult};
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

impl TryFrom<ServiceScheduleVo> for JobScheduleVo {
    type Error = AppError;

    fn try_from(value: ServiceScheduleVo) -> AppResult<Self> {
        let to_utc = |timestamp, field| {
            utc_timestamp::from_chrono(timestamp).map_err(|_| {
                AppError::Internal(format!("调度配置响应的 {field} 无法精确表示为 UTC"))
            })
        };
        Ok(Self {
            id: value.id,
            name: value.name,
            handler_key: value.handler_key,
            cron_expression: value.cron_expression,
            timezone: value.timezone,
            enabled: value.enabled,
            misfire_policy: value.misfire_policy,
            concurrency_policy: value.concurrency_policy,
            max_runtime_seconds: value.max_runtime_seconds,
            next_run_at: value
                .next_run_at
                .map(|instant| to_utc(instant, "next_run_at"))
                .transpose()?,
            last_run_at: value
                .last_run_at
                .map(|instant| to_utc(instant, "last_run_at"))
                .transpose()?,
            version: value.version,
            created_at: to_utc(value.created_at, "created_at")?,
            updated_at: to_utc(value.updated_at, "updated_at")?,
        })
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

impl TryFrom<ServiceExecutionVo> for JobScheduleExecutionVo {
    type Error = AppError;

    fn try_from(value: ServiceExecutionVo) -> AppResult<Self> {
        let to_utc = |timestamp, field| {
            utc_timestamp::from_chrono(timestamp).map_err(|_| {
                AppError::Internal(format!("调度执行记录响应的 {field} 无法精确表示为 UTC"))
            })
        };
        Ok(Self {
            id: value.id,
            schedule_id: value.schedule_id,
            schedule_name: value.schedule_name,
            handler_key: value.handler_key,
            trigger_kind: value.trigger_kind,
            scheduled_for: to_utc(value.scheduled_for, "scheduled_for")?,
            outcome: value.outcome,
            background_job_id: value.background_job_id,
            background_job_status: value.background_job_status,
            detail: value.detail,
            created_at: to_utc(value.created_at, "created_at")?,
        })
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
            next_run_at: Some("1970-01-02T00:00:00Z".parse().unwrap()),
            last_run_at: Some("1969-12-31T23:59:58.120Z".parse().unwrap()),
            version: 7,
            created_at: "1969-12-31T23:59:57.123400Z".parse().unwrap(),
            updated_at: "1969-12-31T23:59:59.123456700Z".parse().unwrap(),
        };
        let encoded =
            serde_json::to_string(&JobScheduleVo::try_from(schedule.clone()).unwrap()).unwrap();
        assert_eq!(encoded, serde_json::to_string(&schedule).unwrap());
        let body: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(body["next_run_at"], "1970-01-02T00:00:00Z");
        assert_eq!(body["last_run_at"], "1969-12-31T23:59:58.120Z");
        assert_eq!(body["created_at"], "1969-12-31T23:59:57.123400Z");
        assert_eq!(body["updated_at"], "1969-12-31T23:59:59.123456700Z");

        schedule.next_run_at = None;
        schedule.last_run_at = None;
        let body =
            serde_json::to_value(JobScheduleVo::try_from(schedule.clone()).unwrap()).unwrap();
        assert_eq!(body.get("next_run_at"), Some(&Value::Null));
        assert_eq!(body.get("last_run_at"), Some(&Value::Null));

        let leap_second = "2016-12-31T23:59:60.123456Z".parse().unwrap();
        schedule.next_run_at = Some(leap_second);
        assert!(
            matches!(JobScheduleVo::try_from(schedule.clone()), Err(AppError::Internal(message))
            if message == "调度配置响应的 next_run_at 无法精确表示为 UTC")
        );
        schedule.next_run_at = None;
        schedule.updated_at = leap_second;
        assert!(
            matches!(JobScheduleVo::try_from(schedule), Err(AppError::Internal(message))
            if message == "调度配置响应的 updated_at 无法精确表示为 UTC")
        );

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
