use chrono::{DateTime, Utc};
use serde::Deserialize;
use utoipa::ToSchema;
use validator::Validate;

use super::{
    password_validation::validate_password_complexity,
    tenant_validation::validate_tenant_identifier,
};

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateTenantDto {
    #[validate(custom(function = "validate_tenant_identifier"))]
    #[schema(pattern = r"^[A-Za-z0-9](?:[A-Za-z0-9_-]{0,62}[A-Za-z0-9])$")]
    pub tenant_id: String,
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    pub domain: Option<String>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: Option<i32>,
    pub max_roles: Option<i32>,
    pub max_storage_mb: Option<i64>,
    pub max_requests_per_min: Option<i32>,
    #[validate(length(min = 2, max = 64))]
    pub admin_username: String,
    #[validate(custom(function = "validate_password_complexity"))]
    #[schema(
        min_length = 8,
        max_length = 72,
        pattern = r"^(?=.*[A-Z])(?=.*[a-z])(?=.*[0-9])(?=.*[^A-Za-z0-9])[!-~]{8,72}$"
    )]
    pub admin_password: String,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTenantDto {
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    pub domain: Option<String>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateTenantStatusDto {
    pub status: String,
}
