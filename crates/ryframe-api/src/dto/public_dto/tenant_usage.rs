use ryframe_application::ports::utc_timestamp;
use ryframe_application::system::platform::{
    QuotaUsage as ServiceQuotaUsage, RequestWindowUsage as ServiceRequestWindowUsage,
    TenantAuxiliaryUsage as ServiceTenantAuxiliaryUsage,
    TenantCapacityVo as ServiceTenantCapacityVo, TenantUsageVo as ServiceTenantUsageVo,
};
use serde::Serialize;
use time::OffsetDateTime;
use utoipa::ToSchema;

use super::organization::TenantVo;

/// 单项租户配额用量。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantQuotaUsageVo {
    pub used: u64,
    /// `None` 表示该资源不受配额限制。
    pub limit: Option<u64>,
    /// 使用率基点，10000 表示 100%；无限制时为 `None`。
    pub percentage_basis_points: Option<u32>,
    pub status: String,
}

impl From<ServiceQuotaUsage> for TenantQuotaUsageVo {
    fn from(value: ServiceQuotaUsage) -> Self {
        Self {
            used: value.used,
            limit: value.limit,
            percentage_basis_points: value.percentage_basis_points,
            status: value.status,
        }
    }
}

/// 租户当前请求限流窗口用量。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantRequestWindowUsageVo {
    /// Redis 不可用时为 `None`。
    pub current: Option<u64>,
    /// `None` 表示租户请求限流未启用。
    pub limit: Option<u64>,
    pub percentage_basis_points: Option<u32>,
    /// Redis 不可用时为 `None`。
    pub remaining_secs: Option<u64>,
    pub status: String,
}

impl From<ServiceRequestWindowUsage> for TenantRequestWindowUsageVo {
    fn from(value: ServiceRequestWindowUsage) -> Self {
        Self {
            current: value.current,
            limit: value.limit,
            percentage_basis_points: value.percentage_basis_points,
            remaining_secs: value.remaining_secs,
            status: value.status,
        }
    }
}

/// 租户后台运行状态汇总。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantAuxiliaryUsageVo {
    pub pending_jobs: u64,
    pub running_jobs: u64,
    pub dead_jobs: u64,
    pub enabled_schedules: u64,
    pub active_user_imports: u64,
    pub cron_enabled: bool,
}

impl From<ServiceTenantAuxiliaryUsage> for TenantAuxiliaryUsageVo {
    fn from(value: ServiceTenantAuxiliaryUsage) -> Self {
        Self {
            pending_jobs: value.pending_jobs,
            running_jobs: value.running_jobs,
            dead_jobs: value.dead_jobs,
            enabled_schedules: value.enabled_schedules,
            active_user_imports: value.active_user_imports,
            cron_enabled: value.cron_enabled,
        }
    }
}

/// 租户容量与当前窗口用量。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantUsageVo {
    pub tenant_id: String,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub calculated_at: OffsetDateTime,
    pub users: TenantQuotaUsageVo,
    pub roles: TenantQuotaUsageVo,
    pub storage: TenantQuotaUsageVo,
    pub request_window: TenantRequestWindowUsageVo,
    pub auxiliary: TenantAuxiliaryUsageVo,
}

impl From<ServiceTenantUsageVo> for TenantUsageVo {
    fn from(value: ServiceTenantUsageVo) -> Self {
        Self {
            tenant_id: value.tenant_id,
            calculated_at: value.calculated_at,
            users: value.users.into(),
            roles: value.roles.into(),
            storage: value.storage.into(),
            request_window: value.request_window.into(),
            auxiliary: value.auxiliary.into(),
        }
    }
}

/// 平台租户分页与详情响应。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantCapacityVo {
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    /// 对外统一使用 `enabled` 或 `disabled`。
    pub status: String,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expire_at: Option<OffsetDateTime>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
    pub expiration_status: String,
    /// 调用者没有 `tenant:usage:list` 权限时为 `None`。
    pub capacity_status: Option<String>,
    /// 调用者没有 `tenant:usage:list` 权限时为 `None`。
    pub usage: Option<TenantUsageVo>,
}

impl From<ServiceTenantCapacityVo> for TenantCapacityVo {
    fn from(value: ServiceTenantCapacityVo) -> Self {
        let tenant = TenantVo::from(value.tenant);
        Self {
            tenant_id: tenant.tenant_id,
            name: tenant.name,
            domain: tenant.domain,
            status: tenant.status,
            expire_at: tenant.expire_at,
            max_users: tenant.max_users,
            max_roles: tenant.max_roles,
            max_storage_mb: tenant.max_storage_mb,
            max_requests_per_min: tenant.max_requests_per_min,
            expiration_status: value.expiration_status,
            capacity_status: value.capacity_status,
            usage: value.usage.map(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ryframe_application::system::platform::TenantVo as ServiceTenantVo;
    use serde_json::Value;
    use utoipa::OpenApi;

    #[test]
    fn tenant_responses_preserve_expiration_utc_precision_and_snapshot_schemas() {
        for expiration in [
            "1969-12-31T23:59:59Z",
            "1970-01-01T00:00:01.120Z",
            "1970-01-01T00:00:02.123400Z",
            "1970-01-01T00:00:03.123456700Z",
        ] {
            let tenant = tenant_fixture(Some(expiration));
            let encoded = serde_json::to_string(&tenant).unwrap();
            let response = serde_json::to_string(&TenantVo::from(tenant)).unwrap();
            assert_eq!(response, encoded);
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["expire_at"], expiration);

            let response = capacity_json(capacity_fixture(Some(expiration), true));
            assert_eq!(response["expire_at"], expiration);
            assert_eq!(response["expiration_status"], "expired");
            assert_eq!(response["capacity_status"], "normal");
            assert_eq!(response["usage"]["tenant_id"], "tenant-a");
        }

        let tenant = tenant_fixture(None);
        let expected = serde_json::to_value(&tenant).unwrap();
        let response = serde_json::to_value(TenantVo::from(tenant)).unwrap();
        assert_eq!(response, expected);
        assert_eq!(response.get("expire_at"), Some(&Value::Null));

        let response = capacity_json(capacity_fixture(None, false));
        assert_eq!(response["expiration_status"], "never");
        for field in ["expire_at", "capacity_status", "usage"] {
            assert_eq!(response.get(field), Some(&Value::Null), "{field}");
        }

        let snapshot: Value =
            serde_json::from_str(include_str!("../../../../../openapi/openapi.json")).unwrap();
        let generated = serde_json::to_value(crate::openapi::ApiDoc::openapi()).unwrap();
        for name in [
            "CreateTenantDto",
            "UpdateTenantDto",
            "TenantVo",
            "TenantCapacityVo",
        ] {
            let path = format!("/components/schemas/{name}");
            assert_eq!(
                generated.pointer(&path),
                Some(snapshot.pointer(&path).expect("快照包含租户响应契约"))
            );
        }
    }

    fn capacity_json(capacity: ServiceTenantCapacityVo) -> Value {
        let mut expected = serde_json::to_value(&capacity).unwrap();
        let fields = expected.as_object_mut().unwrap();
        let Value::Object(tenant) = fields.remove("tenant").unwrap() else {
            panic!("租户容量视图应包含租户对象");
        };
        fields.extend(tenant);
        let response = serde_json::to_value(TenantCapacityVo::from(capacity)).unwrap();
        assert_eq!(response, expected);
        response
    }

    fn tenant_fixture(expire_at: Option<&str>) -> ServiceTenantVo {
        ServiceTenantVo {
            tenant_id: "tenant-a".into(),
            name: "测试租户".into(),
            domain: Some("tenant.example.test".into()),
            status: "enabled".into(),
            expire_at: expire_at.map(|value| {
                OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
                    .unwrap()
            }),
            max_users: 100,
            max_roles: 20,
            max_storage_mb: 1024,
            max_requests_per_min: 1000,
        }
    }

    fn capacity_fixture(expire_at: Option<&str>, include_usage: bool) -> ServiceTenantCapacityVo {
        let tenant = tenant_fixture(expire_at);
        let quota = ServiceQuotaUsage {
            used: 2,
            limit: Some(100),
            percentage_basis_points: Some(200),
            status: "normal".into(),
        };
        let usage = include_usage.then(|| ServiceTenantUsageVo {
            tenant_id: tenant.tenant_id.clone(),
            calculated_at: OffsetDateTime::UNIX_EPOCH,
            users: quota.clone(),
            roles: quota.clone(),
            storage: quota,
            request_window: ServiceRequestWindowUsage {
                current: Some(10),
                limit: Some(1000),
                percentage_basis_points: Some(100),
                remaining_secs: Some(20),
                status: "normal".into(),
            },
            auxiliary: ServiceTenantAuxiliaryUsage {
                pending_jobs: 1,
                running_jobs: 2,
                dead_jobs: 0,
                enabled_schedules: 1,
                active_user_imports: 0,
                cron_enabled: true,
            },
        });
        ServiceTenantCapacityVo {
            tenant,
            expiration_status: if expire_at.is_some() {
                "expired"
            } else {
                "never"
            }
            .into(),
            capacity_status: include_usage.then(|| "normal".into()),
            usage,
        }
    }
}
