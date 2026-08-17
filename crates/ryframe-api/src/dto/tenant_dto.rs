use chrono::{DateTime, Utc};
use ryframe_config::PaginationConfig;
use ryframe_core::ValidatedPageQuery;
use ryframe_http::HttpResult;
use ryframe_service::system::TenantUsagePageParams;
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{
    password_validation::validate_password_complexity,
    tenant_validation::validate_tenant_identifier,
};

const TENANT_CAPACITY_DEFAULT_PAGE_SIZE: u64 = 20;
const TENANT_CAPACITY_MAX_PAGE_SIZE: u64 = 100;

/// 租户启停状态筛选。
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatusFilter {
    Enabled,
    Disabled,
}

impl TenantStatusFilter {
    const fn as_service_value(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

/// 租户到期状态筛选。
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TenantExpirationStatusFilter {
    Active,
    Expiring,
    Expired,
    Never,
}

impl TenantExpirationStatusFilter {
    const fn as_storage_value(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expiring => "expiring",
            Self::Expired => "expired",
            Self::Never => "never",
        }
    }
}

/// 租户容量状态筛选。
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TenantCapacityStatusFilter {
    Normal,
    Warning,
    Critical,
    Exceeded,
    Unlimited,
    Unknown,
}

impl TenantCapacityStatusFilter {
    const fn as_storage_value(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Warning => "warning",
            Self::Critical => "critical",
            Self::Exceeded => "exceeded",
            Self::Unlimited => "unlimited",
            Self::Unknown => "unknown",
        }
    }
}

/// 平台租户容量分页查询参数。
#[derive(Debug, Deserialize, Validate, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct TenantCapacityPageQuery {
    /// 页码，从 1 开始；省略时为 1。
    #[param(minimum = 1)]
    pub page: Option<u64>,
    /// 每页记录数，省略时为 20，最大为 100。
    #[param(minimum = 1, maximum = 100)]
    pub page_size: Option<u64>,
    /// 按租户标识模糊搜索。
    #[validate(length(max = 64))]
    pub tenant_id: Option<String>,
    /// 按租户名称模糊搜索。
    #[validate(length(max = 128))]
    pub name: Option<String>,
    /// 按租户启停状态筛选。
    pub status: Option<TenantStatusFilter>,
    /// 按到期状态筛选。
    pub expiration_status: Option<TenantExpirationStatusFilter>,
    /// 按容量状态筛选；调用者还必须具有 `tenant:usage:list` 权限。
    pub capacity_status: Option<TenantCapacityStatusFilter>,
}

impl TenantCapacityPageQuery {
    /// 判断查询是否请求了只有用量查看者才能使用的容量筛选。
    pub const fn has_capacity_filter(&self) -> bool {
        self.capacity_status.is_some()
    }

    pub fn into_service_params(self) -> HttpResult<(TenantUsagePageParams, ValidatedPageQuery)> {
        let policy = PaginationConfig {
            default_page_size: TENANT_CAPACITY_DEFAULT_PAGE_SIZE,
            max_page_size: TENANT_CAPACITY_MAX_PAGE_SIZE,
        };
        let page = ValidatedPageQuery::from_optional(self.page, self.page_size, &policy)?;
        Ok((
            TenantUsagePageParams {
                tenant_id: self.tenant_id,
                name: self.name,
                status: self.status.map(|value| value.as_service_value().to_owned()),
                expiration_status: self
                    .expiration_status
                    .map(|value| value.as_storage_value().to_owned()),
                capacity_status: self
                    .capacity_status
                    .map(|value| value.as_storage_value().to_owned()),
            },
            page,
        ))
    }

    /// 返回本接口在运行时允许的最大页大小。
    pub const fn max_page_size() -> u64 {
        TENANT_CAPACITY_MAX_PAGE_SIZE
    }
}

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
    /// 已发布产品套餐版本 ID；为避免 JavaScript 精度损失使用十进制字符串。
    #[validate(length(min = 1, max = 20))]
    #[schema(value_type = String, pattern = r"^[1-9][0-9]{0,18}$")]
    pub plan_version_id: String,
    /// 初始租户数据目标稳定键。
    #[validate(length(min = 1, max = 64))]
    pub data_target_key: String,
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
