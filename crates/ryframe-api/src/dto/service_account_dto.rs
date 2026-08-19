use chrono::{DateTime, Utc};
use ryframe_adapters::ValidatedPageQuery;
use ryframe_config::PaginationConfig;
use ryframe_http::HttpResult;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

/// 服务账号、委托与访问审计共用的分页参数。
#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ServiceResourcePageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1)]
    pub page_size: Option<u64>,
}

impl ServiceResourcePageQuery {
    pub fn into_page(self, policy: &PaginationConfig) -> HttpResult<ValidatedPageQuery> {
        Ok(ValidatedPageQuery::from_optional(
            self.page,
            self.page_size,
            policy,
        )?)
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateServiceAccountDto {
    #[validate(length(min = 1, max = 64))]
    #[schema(pattern = r"^[a-z0-9_-]{1,64}$")]
    pub code: String,
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    #[validate(length(max = 500))]
    pub description: Option<String>,
    pub dept_id: Option<String>,
    #[schema(minimum = 1, maximum = 10000)]
    pub max_requests_per_minute: Option<i32>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateServiceAccountDto {
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    #[validate(length(max = 500))]
    pub description: Option<String>,
    pub dept_id: Option<String>,
    #[schema(minimum = 1, maximum = 10000)]
    pub max_requests_per_minute: i32,
}

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAccountStatusDto {
    Enabled,
    Disabled,
}

impl ServiceAccountStatusDto {
    pub const fn as_storage_value(self) -> &'static str {
        match self {
            Self::Enabled => "1",
            Self::Disabled => "0",
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateServiceAccountStatusDto {
    pub status: ServiceAccountStatusDto,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReplaceServiceAccountRolesDto {
    #[validate(length(max = 1000))]
    pub role_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateServiceCredentialDto {
    #[validate(length(min = 1, max = 128))]
    pub label: String,
    #[schema(value_type = String, format = DateTime)]
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateServiceDelegationDto {
    pub service_account_id: String,
    #[validate(length(min = 1, max = 16))]
    pub capability_keys: Vec<String>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<DateTime<Utc>>,
    #[validate(length(min = 1, max = 500))]
    pub reason: String,
}
