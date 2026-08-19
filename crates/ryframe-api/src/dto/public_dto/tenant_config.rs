use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ryframe_application::system::{
    TenantConfigBundleSummaryVo as ServiceBundleSummary, TenantConfigBundleVo as ServiceBundle,
    TenantConfigTransferItemVo as ServiceItem, TenantConfigTransferVo as ServiceTransfer,
};
use serde::Serialize;
use utoipa::ToSchema;

/// 配置包的安全公开视图，不包含对象路径或数据库内部标识。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantConfigBundleVo {
    pub id: String,
    pub origin: String,
    pub source_tenant_key: String,
    pub source_tenant_name: String,
    pub package_schema_version: String,
    pub source_app_version: String,
    pub sha256: Option<String>,
    pub resource_counts: BTreeMap<String, u64>,
    pub item_count: i32,
    pub status: String,
    pub error_summary: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ServiceBundle> for TenantConfigBundleVo {
    fn from(value: ServiceBundle) -> Self {
        Self {
            id: value.id,
            origin: value.origin,
            source_tenant_key: value.source_tenant_key,
            source_tenant_name: value.source_tenant_name,
            package_schema_version: value.package_schema_version,
            source_app_version: value.source_app_version,
            sha256: value.sha256,
            resource_counts: value.resource_counts,
            item_count: value.item_count,
            status: value.status,
            error_summary: value.error_summary,
            expires_at: value.expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

/// 配置迁移中关联配置包的安全摘要，不包含数据库内部标识。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantConfigBundleSummaryVo {
    pub origin: String,
    pub source_tenant_key: String,
    pub source_tenant_name: String,
    pub package_schema_version: String,
    pub source_app_version: String,
    pub sha256: Option<String>,
    pub resource_counts: BTreeMap<String, u64>,
    pub item_count: i32,
    pub status: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<ServiceBundleSummary> for TenantConfigBundleSummaryVo {
    fn from(value: ServiceBundleSummary) -> Self {
        Self {
            origin: value.origin,
            source_tenant_key: value.source_tenant_key,
            source_tenant_name: value.source_tenant_name,
            package_schema_version: value.package_schema_version,
            source_app_version: value.source_app_version,
            sha256: value.sha256,
            resource_counts: value.resource_counts,
            item_count: value.item_count,
            status: value.status,
            expires_at: value.expires_at,
            created_at: value.created_at,
        }
    }
}

/// 一次目标租户配置预览、应用或回滚的公开视图。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantConfigTransferVo {
    pub id: String,
    pub bundle_summary: TenantConfigBundleSummaryVo,
    pub status: String,
    pub target_configuration_version: i64,
    pub target_authorization_epoch: String,
    pub plan_hash: Option<String>,
    pub preview_calculated_at: Option<DateTime<Utc>>,
    pub change_counts: BTreeMap<String, u64>,
    pub error_summary: Option<String>,
    pub applied_configuration_version: Option<i64>,
    pub applied_authorization_epoch: Option<String>,
    pub rollback_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ServiceTransfer> for TenantConfigTransferVo {
    fn from(value: ServiceTransfer) -> Self {
        Self {
            id: value.id,
            bundle_summary: value.bundle_summary.into(),
            status: value.status,
            target_configuration_version: value.target_configuration_version,
            target_authorization_epoch: value.target_authorization_epoch.to_string(),
            plan_hash: value.plan_hash,
            preview_calculated_at: value.preview_calculated_at,
            change_counts: value.change_counts,
            error_summary: value.error_summary,
            applied_configuration_version: value.applied_configuration_version,
            applied_authorization_epoch: value
                .applied_authorization_epoch
                .map(|epoch| epoch.to_string()),
            rollback_expires_at: value.rollback_expires_at,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

/// 配置迁移预览和执行的逐项安全结果。
#[derive(Debug, Serialize, ToSchema)]
pub struct TenantConfigTransferItemVo {
    pub resource_type: String,
    pub stable_key: String,
    pub display_name: String,
    pub action: String,
    pub outcome: String,
    pub detail_code: Option<String>,
    pub detail: Option<String>,
}

impl From<ServiceItem> for TenantConfigTransferItemVo {
    fn from(value: ServiceItem) -> Self {
        Self {
            resource_type: value.resource_type,
            stable_key: value.stable_key,
            display_name: value.display_name,
            action: value.action,
            outcome: value.outcome,
            detail_code: value.detail_code,
            detail: value.detail,
        }
    }
}
