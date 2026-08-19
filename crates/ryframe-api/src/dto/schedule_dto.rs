use crate::http::HttpResult;
use ryframe_application::{
    CreateJobSchedule, JobScheduleExecutionListParams, JobScheduleListParams, UpdateJobSchedule,
};
use ryframe_config::PaginationConfig;
use ryframe_kernel::ValidatedPageQuery;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicyDto {
    Skip,
    FireOnce,
}

impl MisfirePolicyDto {
    fn as_str(self) -> &'static str {
        match self {
            Self::Skip => "skip",
            Self::FireOnce => "fire_once",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyPolicyDto {
    Forbid,
    Allow,
}

impl ConcurrencyPolicyDto {
    fn as_str(self) -> &'static str {
        match self {
            Self::Forbid => "forbid",
            Self::Allow => "allow",
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SchedulePreviewRequest {
    pub cron_expression: String,
    pub timezone: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_misfire_policy")]
    pub misfire_policy: MisfirePolicyDto,
    #[serde(default = "default_concurrency_policy")]
    pub concurrency_policy: ConcurrencyPolicyDto,
    #[serde(default = "default_max_runtime_seconds")]
    pub max_runtime_seconds: i32,
}

impl From<CreateScheduleRequest> for CreateJobSchedule {
    fn from(value: CreateScheduleRequest) -> Self {
        Self {
            name: value.name,
            handler_key: value.handler_key,
            cron_expression: value.cron_expression,
            timezone: value.timezone,
            enabled: value.enabled,
            misfire_policy: value.misfire_policy.as_str().to_owned(),
            concurrency_policy: value.concurrency_policy.as_str().to_owned(),
            max_runtime_seconds: value.max_runtime_seconds,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateScheduleRequest {
    pub version: i64,
    pub name: String,
    pub handler_key: String,
    pub cron_expression: String,
    pub timezone: String,
    pub enabled: bool,
    pub misfire_policy: MisfirePolicyDto,
    pub concurrency_policy: ConcurrencyPolicyDto,
    pub max_runtime_seconds: i32,
}

impl From<UpdateScheduleRequest> for UpdateJobSchedule {
    fn from(value: UpdateScheduleRequest) -> Self {
        Self {
            version: value.version,
            name: value.name,
            handler_key: value.handler_key,
            cron_expression: value.cron_expression,
            timezone: value.timezone,
            enabled: value.enabled,
            misfire_policy: value.misfire_policy.as_str().to_owned(),
            concurrency_policy: value.concurrency_policy.as_str().to_owned(),
            max_runtime_seconds: value.max_runtime_seconds,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateScheduleStatusRequest {
    pub version: i64,
    pub enabled: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleVersionRequest {
    pub version: i64,
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct SchedulePageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1)]
    pub page_size: Option<u64>,
    pub name: Option<String>,
    pub handler_key: Option<String>,
    pub enabled: Option<bool>,
}

impl SchedulePageQuery {
    pub fn into_service_params(
        self,
        policy: &PaginationConfig,
    ) -> HttpResult<JobScheduleListParams> {
        Ok(JobScheduleListParams {
            page: ValidatedPageQuery::from_optional(self.page, self.page_size, policy)?,
            name: self.name,
            handler_key: self.handler_key,
            enabled: self.enabled,
        })
    }
}

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ScheduleExecutionPageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1)]
    pub page_size: Option<u64>,
    pub trigger_kind: Option<String>,
    pub outcome: Option<String>,
    pub background_job_status: Option<String>,
}

impl ScheduleExecutionPageQuery {
    pub fn into_service_params(
        self,
        policy: &PaginationConfig,
    ) -> HttpResult<JobScheduleExecutionListParams> {
        Ok(JobScheduleExecutionListParams {
            page: ValidatedPageQuery::from_optional(self.page, self.page_size, policy)?,
            trigger_kind: self.trigger_kind,
            outcome: self.outcome,
            background_job_status: self.background_job_status,
        })
    }
}

const fn default_enabled() -> bool {
    true
}

const fn default_misfire_policy() -> MisfirePolicyDto {
    MisfirePolicyDto::FireOnce
}

const fn default_concurrency_policy() -> ConcurrencyPolicyDto {
    ConcurrencyPolicyDto::Forbid
}

const fn default_max_runtime_seconds() -> i32 {
    900
}
