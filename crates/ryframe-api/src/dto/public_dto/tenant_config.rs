use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ryframe_application::ports::utc_timestamp;
use ryframe_application::system::platform::{
    TenantConfigBundleSummaryVo as ServiceBundleSummary, TenantConfigBundleVo as ServiceBundle,
    TenantConfigTransferItemVo as ServiceItem, TenantConfigTransferVo as ServiceTransfer,
};
use ryframe_kernel::AppError;
use serde::Serialize;
use time::OffsetDateTime;
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
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: OffsetDateTime,
}

impl TryFrom<ServiceBundle> for TenantConfigBundleVo {
    type Error = AppError;

    fn try_from(value: ServiceBundle) -> Result<Self, Self::Error> {
        Ok(Self {
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
            expires_at: optional_response_timestamp(value.expires_at, "expires_at")?,
            created_at: response_timestamp(value.created_at, "created_at")?,
            updated_at: response_timestamp(value.updated_at, "updated_at")?,
        })
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
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
}

impl TryFrom<ServiceBundleSummary> for TenantConfigBundleSummaryVo {
    type Error = AppError;

    fn try_from(value: ServiceBundleSummary) -> Result<Self, Self::Error> {
        Ok(Self {
            origin: value.origin,
            source_tenant_key: value.source_tenant_key,
            source_tenant_name: value.source_tenant_name,
            package_schema_version: value.package_schema_version,
            source_app_version: value.source_app_version,
            sha256: value.sha256,
            resource_counts: value.resource_counts,
            item_count: value.item_count,
            status: value.status,
            expires_at: optional_response_timestamp(value.expires_at, "bundle_summary.expires_at")?,
            created_at: response_timestamp(value.created_at, "bundle_summary.created_at")?,
        })
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
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub preview_calculated_at: Option<OffsetDateTime>,
    pub change_counts: BTreeMap<String, u64>,
    pub error_summary: Option<String>,
    pub applied_configuration_version: Option<i64>,
    pub applied_authorization_epoch: Option<String>,
    #[serde(serialize_with = "utc_timestamp::serialize_optional")]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub rollback_expires_at: Option<OffsetDateTime>,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub created_at: OffsetDateTime,
    #[serde(serialize_with = "utc_timestamp::serialize")]
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: OffsetDateTime,
}

impl TryFrom<ServiceTransfer> for TenantConfigTransferVo {
    type Error = AppError;

    fn try_from(value: ServiceTransfer) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            bundle_summary: value.bundle_summary.try_into()?,
            status: value.status,
            target_configuration_version: value.target_configuration_version,
            target_authorization_epoch: value.target_authorization_epoch.to_string(),
            plan_hash: value.plan_hash,
            preview_calculated_at: optional_response_timestamp(
                value.preview_calculated_at,
                "preview_calculated_at",
            )?,
            change_counts: value.change_counts,
            error_summary: value.error_summary,
            applied_configuration_version: value.applied_configuration_version,
            applied_authorization_epoch: value
                .applied_authorization_epoch
                .map(|epoch| epoch.to_string()),
            rollback_expires_at: optional_response_timestamp(
                value.rollback_expires_at,
                "rollback_expires_at",
            )?,
            created_at: response_timestamp(value.created_at, "created_at")?,
            updated_at: response_timestamp(value.updated_at, "updated_at")?,
        })
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

fn response_timestamp(value: DateTime<Utc>, field: &str) -> Result<OffsetDateTime, AppError> {
    utc_timestamp::from_chrono(value)
        .map_err(|_| AppError::Internal(format!("租户配置响应的 {field} 无法精确表示为 UTC")))
}

fn optional_response_timestamp(
    value: Option<DateTime<Utc>>,
    field: &str,
) -> Result<Option<OffsetDateTime>, AppError> {
    value
        .map(|instant| response_timestamp(instant, field))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use utoipa::OpenApi;

    #[test]
    fn tenant_config_responses_preserve_utc_precision_and_snapshot_schemas() {
        let mut bundle = bundle_fixture();
        let mut transfer = transfer_fixture(&bundle);
        let encoded =
            serde_json::to_string(&TenantConfigBundleVo::try_from(bundle.clone()).unwrap())
                .unwrap();
        assert_eq!(encoded, serde_json::to_string(&bundle).unwrap());
        let bundle_json: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(bundle_json["created_at"], "1969-12-31T23:59:59Z");
        assert_eq!(bundle_json["updated_at"], "1970-01-01T00:00:01.120Z");
        assert_eq!(bundle_json["expires_at"], "1970-01-01T00:00:06.123400Z");

        let actual = transfer_json(&transfer);
        assert_eq!(
            actual["bundle_summary"]["expires_at"],
            bundle_json["expires_at"]
        );
        assert_eq!(
            actual["bundle_summary"]["created_at"],
            bundle_json["created_at"]
        );
        assert_eq!(actual["preview_calculated_at"], "1970-01-01T00:00:03.120Z");
        assert_eq!(actual["rollback_expires_at"], "1970-01-01T00:00:08.123400Z");
        assert_eq!(actual["created_at"], "1970-01-01T00:00:02Z");
        assert_eq!(actual["updated_at"], "1970-01-01T00:00:04.123456700Z");

        bundle.expires_at = None;
        let actual =
            serde_json::to_value(TenantConfigBundleVo::try_from(bundle.clone()).unwrap()).unwrap();
        assert_eq!(actual, serde_json::to_value(&bundle).unwrap());
        assert_eq!(actual.get("expires_at"), Some(&Value::Null));
        transfer.bundle_summary.expires_at = None;
        transfer.preview_calculated_at = None;
        transfer.rollback_expires_at = None;
        transfer.applied_authorization_epoch = None;
        let actual = transfer_json(&transfer);
        assert_eq!(
            actual.pointer("/bundle_summary/expires_at"),
            Some(&Value::Null)
        );
        for field in [
            "preview_calculated_at",
            "rollback_expires_at",
            "applied_authorization_epoch",
        ] {
            assert_eq!(actual.get(field), Some(&Value::Null), "{field}");
        }

        let leap_second = DateTime::from_timestamp(1_483_228_799, 1_123_456_000).unwrap();
        bundle.expires_at = Some(leap_second);
        assert_timestamp_error(TenantConfigBundleVo::try_from(bundle), "expires_at");
        let mut invalid_summary = transfer.clone();
        invalid_summary.bundle_summary.created_at = leap_second;
        assert_timestamp_error(
            TenantConfigTransferVo::try_from(invalid_summary),
            "bundle_summary.created_at",
        );
        transfer.updated_at = leap_second;
        assert_timestamp_error(TenantConfigTransferVo::try_from(transfer), "updated_at");

        let snapshot: Value =
            serde_json::from_str(include_str!("../../../../../openapi/openapi.json")).unwrap();
        let generated = serde_json::to_value(crate::openapi::ApiDoc::openapi()).unwrap();
        for name in [
            "TenantConfigBundleVo",
            "TenantConfigBundleSummaryVo",
            "TenantConfigTransferVo",
        ] {
            let path = format!("/components/schemas/{name}");
            assert_eq!(
                generated.pointer(&path),
                Some(snapshot.pointer(&path).expect("快照包含租户配置响应契约"))
            );
        }
    }

    fn transfer_json(transfer: &ServiceTransfer) -> Value {
        let actual =
            serde_json::to_value(TenantConfigTransferVo::try_from(transfer.clone()).unwrap())
                .unwrap();
        let mut expected = serde_json::to_value(transfer).unwrap();
        expected["target_authorization_epoch"] =
            json!(transfer.target_authorization_epoch.to_string());
        expected["applied_authorization_epoch"] = json!(
            transfer
                .applied_authorization_epoch
                .map(|epoch| epoch.to_string())
        );
        assert_eq!(actual, expected);
        actual
    }

    fn assert_timestamp_error<T: std::fmt::Debug>(result: Result<T, AppError>, field: &str) {
        let expected = format!("租户配置响应的 {field} 无法精确表示为 UTC");
        assert!(matches!(result.unwrap_err(), AppError::Internal(message) if message == expected));
    }

    fn bundle_fixture() -> ServiceBundle {
        ServiceBundle {
            id: "7".into(),
            origin: "uploaded".into(),
            source_tenant_key: "source".into(),
            source_tenant_name: "来源租户".into(),
            package_schema_version: "1".into(),
            source_app_version: "0.10.0".into(),
            sha256: Some("a".repeat(64)),
            resource_counts: BTreeMap::from([("role".into(), 2)]),
            item_count: 2,
            status: "succeeded".into(),
            error_summary: None,
            expires_at: DateTime::from_timestamp(6, 123_400_000),
            created_at: DateTime::from_timestamp(-1, 0).unwrap(),
            updated_at: DateTime::from_timestamp(1, 120_000_000).unwrap(),
        }
    }

    fn transfer_fixture(bundle: &ServiceBundle) -> ServiceTransfer {
        ServiceTransfer {
            id: "11".into(),
            bundle_summary: ServiceBundleSummary {
                origin: bundle.origin.clone(),
                source_tenant_key: bundle.source_tenant_key.clone(),
                source_tenant_name: bundle.source_tenant_name.clone(),
                package_schema_version: bundle.package_schema_version.clone(),
                source_app_version: bundle.source_app_version.clone(),
                sha256: bundle.sha256.clone(),
                resource_counts: bundle.resource_counts.clone(),
                item_count: bundle.item_count,
                status: bundle.status.clone(),
                expires_at: bundle.expires_at,
                created_at: bundle.created_at,
            },
            status: "applied".into(),
            target_configuration_version: 13,
            target_authorization_epoch: 14,
            plan_hash: Some("b".repeat(64)),
            preview_calculated_at: DateTime::from_timestamp(3, 120_000_000),
            change_counts: BTreeMap::from([("create".into(), 2)]),
            error_summary: None,
            applied_configuration_version: Some(15),
            applied_authorization_epoch: Some(16),
            rollback_expires_at: DateTime::from_timestamp(8, 123_400_000),
            created_at: DateTime::from_timestamp(2, 0).unwrap(),
            updated_at: DateTime::from_timestamp(4, 123_456_700).unwrap(),
        }
    }
}
