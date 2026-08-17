use std::collections::BTreeMap;

use ryframe_kernel::AppError;
use ryframe_service::system::{
    CapabilityCatalogVo as ServiceCapabilityCatalogVo,
    CapabilityOverrideVo as ServiceCapabilityOverrideVo,
    EffectiveCapabilityVo as ServiceEffectiveCapabilityVo,
    ProductCapabilityChangeVo as ServiceProductCapabilityChangeVo,
    ProductCapabilityVo as ServiceProductCapabilityVo,
    ProductChangePreviewVo as ServiceProductChangePreviewVo,
    ProductContextVo as ServiceProductContextVo,
    ProductPlanVersionVo as ServiceProductPlanVersionVo, ProductPlanVo as ServiceProductPlanVo,
    SessionCapabilityVo as ServiceSessionCapabilityVo,
    SessionProductContextVo as ServiceSessionProductContextVo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use validator::Validate;

use super::fixed_value::{EffectiveCapabilitySource, ProductPlanStatus, ProductPlanVersionStatus};

const PRODUCT_PLAN_DEFAULT_PAGE_SIZE: u64 = 20;
const PRODUCT_PLAN_MAX_PAGE_SIZE: u64 = 100;

pub(crate) fn into_json_object(value: BTreeMap<String, Value>) -> Value {
    Value::Object(value.into_iter().collect())
}

fn checked_json_object(
    value: Value,
    field: &'static str,
) -> Result<BTreeMap<String, Value>, AppError> {
    match value {
        Value::Object(object) => Ok(object.into_iter().collect()),
        _ => {
            tracing::error!(field, "服务层返回了非对象 Capability 配置");
            Err(AppError::Internal(format!("{field} 必须是 JSON object")))
        }
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct ProductPlanPageQuery {
    #[param(minimum = 1)]
    pub page: Option<u64>,
    #[param(minimum = 1, maximum = 100)]
    pub page_size: Option<u64>,
}

impl ProductPlanPageQuery {
    pub fn validate_page(
        &self,
    ) -> Result<ryframe_core::ValidatedPageQuery, ryframe_kernel::AppError> {
        ryframe_core::ValidatedPageQuery::from_optional(
            self.page,
            self.page_size,
            &ryframe_config::PaginationConfig {
                default_page_size: PRODUCT_PLAN_DEFAULT_PAGE_SIZE,
                max_page_size: PRODUCT_PLAN_MAX_PAGE_SIZE,
            },
        )
    }

    pub const fn max_page_size() -> u64 {
        PRODUCT_PLAN_MAX_PAGE_SIZE
    }
}

#[derive(Clone, Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilitySnapshotDto {
    #[validate(length(min = 1, max = 96))]
    pub capability_code: String,
    #[validate(length(min = 1, max = 64))]
    pub variant_code: String,
    #[validate(range(min = 1))]
    pub schema_version: i32,
    pub config: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityOverrideDto {
    #[validate(length(min = 1, max = 96))]
    pub capability_code: String,
    pub enabled: bool,
    #[validate(length(min = 1, max = 64))]
    pub variant_code: String,
    #[validate(range(min = 1))]
    pub schema_version: i32,
    pub config: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProductPlanDto {
    #[validate(length(min = 2, max = 64))]
    pub key: String,
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    #[validate(length(max = 500))]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateProductPlanDto {
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    #[validate(length(max = 500))]
    pub description: Option<String>,
    pub status: ProductPlanStatus,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateProductPlanVersionDto {
    #[validate(length(min = 1, max = 128))]
    pub name: String,
    #[validate(length(max = 500))]
    pub description: Option<String>,
    #[validate(nested)]
    pub capabilities: Vec<CapabilitySnapshotDto>,
}

pub type UpdateProductPlanVersionDto = CreateProductPlanVersionDto;

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductChangePreviewDto {
    #[validate(length(min = 1, max = 20))]
    #[schema(value_type = String, pattern = r"^[1-9][0-9]{0,18}$")]
    pub plan_version_id: String,
    #[serde(default)]
    #[validate(nested)]
    pub overrides: Vec<CapabilityOverrideDto>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ProductChangeApplyDto {
    #[validate(length(min = 1, max = 20))]
    #[schema(value_type = String, pattern = r"^[1-9][0-9]{0,18}$")]
    pub plan_version_id: String,
    #[serde(default)]
    #[validate(nested)]
    pub overrides: Vec<CapabilityOverrideDto>,
    #[validate(length(min = 1, max = 20))]
    #[schema(value_type = String, pattern = r"^[1-9][0-9]{0,18}$")]
    pub preview_runtime_epoch: String,
    #[validate(length(equal = 64))]
    #[schema(pattern = r"^[a-f0-9]{64}$")]
    pub plan_hash: String,
    #[validate(length(max = 500))]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CapabilityVariantVo {
    pub code: String,
    pub schema_version: i32,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
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

impl From<ServiceCapabilityCatalogVo> for CapabilityCatalogVo {
    fn from(value: ServiceCapabilityCatalogVo) -> Self {
        Self {
            code: value.code,
            name: value.name,
            description: value.description,
            affects_authorization: value.affects_authorization,
            dependencies: value.dependencies,
            conflicts: value.conflicts,
            route_keys: value.route_keys,
            permission_codes: value.permission_codes,
            default_admin_permissions: value.default_admin_permissions,
            deployment_dependencies: value.deployment_dependencies,
            client_config_fields: value.client_config_fields,
            deployment_available: value.deployment_available,
            variants: value
                .variants
                .into_iter()
                .map(|variant| CapabilityVariantVo {
                    code: variant.code,
                    schema_version: variant.schema_version,
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ProductCapabilityVo {
    pub capability_code: String,
    pub variant_code: String,
    pub schema_version: i32,
    pub config: BTreeMap<String, Value>,
}

impl TryFrom<ServiceProductCapabilityVo> for ProductCapabilityVo {
    type Error = AppError;

    fn try_from(value: ServiceProductCapabilityVo) -> Result<Self, Self::Error> {
        Ok(Self {
            capability_code: value.capability_code,
            variant_code: value.variant_code,
            schema_version: value.schema_version,
            config: checked_json_object(value.config, "product_capability.config")?,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ProductPlanVersionVo {
    pub id: String,
    pub version: i32,
    pub name: String,
    pub description: Option<String>,
    pub status: ProductPlanVersionStatus,
    pub created_by: String,
    pub published_by: Option<String>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    pub capabilities: Vec<ProductCapabilityVo>,
}

impl TryFrom<ServiceProductPlanVersionVo> for ProductPlanVersionVo {
    type Error = AppError;

    fn try_from(value: ServiceProductPlanVersionVo) -> Result<Self, Self::Error> {
        let status = ProductPlanVersionStatus::try_from(value.status.as_str())?;
        let capabilities = value
            .capabilities
            .into_iter()
            .map(ProductCapabilityVo::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id: value.id,
            version: value.version,
            name: value.name,
            description: value.description,
            status,
            created_by: value.created_by,
            published_by: value.published_by,
            published_at: value.published_at,
            capabilities,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ProductPlanVo {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub status: ProductPlanStatus,
    pub created_by: String,
    pub versions: Vec<ProductPlanVersionVo>,
}

impl TryFrom<ServiceProductPlanVo> for ProductPlanVo {
    type Error = AppError;

    fn try_from(value: ServiceProductPlanVo) -> Result<Self, Self::Error> {
        let status = ProductPlanStatus::try_from(value.status.as_str())?;
        let versions = value
            .versions
            .into_iter()
            .map(ProductPlanVersionVo::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id: value.id,
            key: value.key,
            name: value.name,
            description: value.description,
            status,
            created_by: value.created_by,
            versions,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EffectiveCapabilityVo {
    pub capability_code: String,
    pub name: String,
    pub enabled: bool,
    pub entitled: bool,
    pub deployment_enabled: bool,
    pub source: EffectiveCapabilitySource,
    pub variant_code: Option<String>,
    pub schema_version: Option<i32>,
    pub config: Option<BTreeMap<String, Value>>,
}

impl TryFrom<ServiceEffectiveCapabilityVo> for EffectiveCapabilityVo {
    type Error = AppError;

    fn try_from(value: ServiceEffectiveCapabilityVo) -> Result<Self, Self::Error> {
        let source = EffectiveCapabilitySource::try_from(value.source.as_str())?;
        let config = value
            .config
            .map(|config| checked_json_object(config, "effective_capability.config"))
            .transpose()?;
        Ok(Self {
            capability_code: value.capability_code,
            name: value.name,
            enabled: value.enabled,
            entitled: value.entitled,
            deployment_enabled: value.deployment_enabled,
            source,
            variant_code: value.variant_code,
            schema_version: value.schema_version,
            config,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CapabilityOverrideVo {
    pub capability_code: String,
    pub enabled: bool,
    pub variant_code: String,
    pub schema_version: i32,
    pub config: BTreeMap<String, Value>,
    pub reason: Option<String>,
    pub changed_by: Option<String>,
}

impl TryFrom<ServiceCapabilityOverrideVo> for CapabilityOverrideVo {
    type Error = AppError;

    fn try_from(value: ServiceCapabilityOverrideVo) -> Result<Self, Self::Error> {
        Ok(Self {
            capability_code: value.capability_code,
            enabled: value.enabled,
            variant_code: value.variant_code,
            schema_version: value.schema_version,
            config: checked_json_object(value.config, "capability_override.config")?,
            reason: value.reason,
            changed_by: value.changed_by,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
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

impl TryFrom<ServiceProductContextVo> for ProductContextVo {
    type Error = AppError;

    fn try_from(value: ServiceProductContextVo) -> Result<Self, Self::Error> {
        let capabilities = value
            .capabilities
            .into_iter()
            .map(EffectiveCapabilityVo::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        let overrides = value
            .overrides
            .into_iter()
            .map(CapabilityOverrideVo::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            tenant_id: value.tenant_id,
            runtime_epoch: value.runtime_epoch,
            plan_key: value.plan_key,
            plan_name: value.plan_name,
            plan_version_id: value.plan_version_id,
            plan_version: value.plan_version,
            capabilities,
            overrides,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ProductCapabilityChangeVo {
    pub capability_code: String,
    pub before: EffectiveCapabilityVo,
    pub after: EffectiveCapabilityVo,
}

impl TryFrom<ServiceProductCapabilityChangeVo> for ProductCapabilityChangeVo {
    type Error = AppError;

    fn try_from(value: ServiceProductCapabilityChangeVo) -> Result<Self, Self::Error> {
        Ok(Self {
            capability_code: value.capability_code,
            before: value.before.try_into()?,
            after: value.after.try_into()?,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
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

impl TryFrom<ServiceProductChangePreviewVo> for ProductChangePreviewVo {
    type Error = AppError;

    fn try_from(value: ServiceProductChangePreviewVo) -> Result<Self, Self::Error> {
        let capability_changes = value
            .capability_changes
            .into_iter()
            .map(ProductCapabilityChangeVo::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            tenant_id: value.tenant_id,
            runtime_epoch: value.runtime_epoch,
            plan_hash: value.plan_hash,
            current: value.current.try_into()?,
            target: value.target.try_into()?,
            capability_additions: value.capability_additions,
            capability_removals: value.capability_removals,
            capability_changes,
            menu_additions: value.menu_additions,
            menu_removals: value.menu_removals,
            permission_additions: value.permission_additions,
            permission_removals: value.permission_removals,
            warnings: value.warnings,
        })
    }
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SessionCapabilityVo {
    pub code: String,
    pub variant: String,
    pub schema_version: i32,
    pub client_config: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SessionProductContextVo {
    pub authorization_epoch: String,
    pub runtime_epoch: String,
    pub capabilities: Vec<SessionCapabilityVo>,
}

impl TryFrom<ServiceSessionProductContextVo> for SessionProductContextVo {
    type Error = AppError;

    fn try_from(value: ServiceSessionProductContextVo) -> Result<Self, Self::Error> {
        let capabilities = value
            .capabilities
            .into_iter()
            .map(|capability: ServiceSessionCapabilityVo| {
                Ok(SessionCapabilityVo {
                    code: capability.code,
                    variant: capability.variant,
                    schema_version: capability.schema_version,
                    client_config: checked_json_object(
                        capability.client_config,
                        "session_capability.client_config",
                    )?,
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        Ok(Self {
            authorization_epoch: value.authorization_epoch.to_string(),
            runtime_epoch: value.runtime_epoch,
            capabilities,
        })
    }
}
