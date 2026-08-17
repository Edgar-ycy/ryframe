use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::Json;
use serde::{Deserialize, Serialize};

/// 配置包声明的能力运行时兼容要求。它只描述包内资源所依赖的能力，
/// 不代表、也不能修改目标租户的套餐或覆盖配置。
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityRequirement {
    pub code: String,
    pub variant: String,
    pub schema_version: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapabilityVariantVo {
    pub code: String,
    pub schema_version: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapabilityCatalogVo {
    pub code: String,
    pub name: String,
    pub description: String,
    pub affects_authorization: bool,
    pub dependencies: Vec<String>,
    pub conflicts: Vec<String>,
    pub route_keys: Vec<String>,
    pub permission_codes: Vec<String>,
    pub default_admin_permissions: Vec<String>,
    pub deployment_dependencies: Vec<String>,
    pub client_config_fields: Vec<String>,
    pub deployment_available: bool,
    pub variants: Vec<CapabilityVariantVo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductCapabilityVo {
    pub capability_code: String,
    pub variant_code: String,
    pub schema_version: i32,
    pub config: Json,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductPlanVersionVo {
    pub id: String,
    pub version: i32,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: String,
    pub published_by: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub capabilities: Vec<ProductCapabilityVo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductPlanVo {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub created_by: String,
    pub versions: Vec<ProductPlanVersionVo>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct EffectiveCapabilityVo {
    pub capability_code: String,
    pub name: String,
    pub enabled: bool,
    pub entitled: bool,
    pub deployment_enabled: bool,
    pub source: String,
    pub variant_code: Option<String>,
    pub schema_version: Option<i32>,
    pub config: Option<Json>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductContextVo {
    pub tenant_id: String,
    pub runtime_epoch: String,
    pub plan_key: String,
    pub plan_name: String,
    pub plan_version_id: String,
    pub plan_version: i32,
    pub capabilities: Vec<EffectiveCapabilityVo>,
    pub overrides: Vec<CapabilityOverrideVo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapabilityOverrideVo {
    pub capability_code: String,
    pub enabled: bool,
    pub variant_code: String,
    pub schema_version: i32,
    pub config: Json,
    pub reason: Option<String>,
    pub changed_by: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionCapabilityVo {
    pub code: String,
    pub variant: String,
    pub schema_version: i32,
    pub client_config: Json,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionProductContextVo {
    pub authorization_epoch: i32,
    pub runtime_epoch: String,
    pub capabilities: Vec<SessionCapabilityVo>,
}

#[derive(Clone, Debug)]
pub struct CapabilitySnapshotInput {
    pub capability_code: String,
    pub variant_code: String,
    pub schema_version: i32,
    pub config: Json,
}

#[derive(Clone, Debug)]
pub struct CapabilityOverrideInput {
    pub capability_code: String,
    pub enabled: bool,
    pub variant_code: String,
    pub schema_version: i32,
    pub config: Json,
}

#[derive(Clone, Debug)]
pub struct CreateProductPlanCommand {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UpdateProductPlanCommand {
    pub name: String,
    pub description: Option<String>,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct CreateProductPlanVersionCommand {
    pub name: String,
    pub description: Option<String>,
    pub capabilities: Vec<CapabilitySnapshotInput>,
}

#[derive(Clone, Debug)]
pub struct UpdateProductPlanVersionCommand {
    pub name: String,
    pub description: Option<String>,
    pub capabilities: Vec<CapabilitySnapshotInput>,
}

#[derive(Clone, Debug)]
pub struct ProductChangeTarget {
    pub plan_version_id: i64,
    pub overrides: Vec<CapabilityOverrideInput>,
}

#[derive(Clone, Debug)]
pub struct ApplyProductChangeCommand {
    pub target: ProductChangeTarget,
    pub preview_runtime_epoch: i64,
    pub plan_hash: String,
    pub reason: Option<String>,
    pub capability_override_allowed: bool,
}

#[derive(Clone, Debug)]
pub struct ProvisioningCapabilityResources {
    pub enabled_route_keys: Vec<String>,
    pub enabled_permission_codes: Vec<String>,
    pub managed_route_keys: Vec<String>,
    pub managed_permission_codes: Vec<String>,
    pub default_admin_permissions: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductCapabilityChangeVo {
    pub capability_code: String,
    pub before: EffectiveCapabilityVo,
    pub after: EffectiveCapabilityVo,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProductChangePreviewVo {
    pub tenant_id: String,
    pub runtime_epoch: String,
    pub plan_hash: String,
    pub current: ProductContextVo,
    pub target: ProductContextVo,
    pub capability_additions: Vec<String>,
    pub capability_removals: Vec<String>,
    pub capability_changes: Vec<ProductCapabilityChangeVo>,
    pub menu_additions: Vec<String>,
    pub menu_removals: Vec<String>,
    pub permission_additions: Vec<String>,
    pub permission_removals: Vec<String>,
    pub warnings: Vec<String>,
}

pub(super) struct ProductChangeDiff {
    pub(super) capability_additions: Vec<String>,
    pub(super) capability_removals: Vec<String>,
    pub(super) capability_changes: Vec<ProductCapabilityChangeVo>,
    pub(super) menu_additions: Vec<String>,
    pub(super) menu_removals: Vec<String>,
    pub(super) permission_additions: Vec<String>,
    pub(super) permission_removals: Vec<String>,
    pub(super) warnings: Vec<String>,
}
