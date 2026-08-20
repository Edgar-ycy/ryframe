use std::collections::{BTreeSet, HashSet};

use super::{
    CAPABILITY_CATALOG, CapabilityRequirement, ProductContextVo, ProductService,
    ProvisioningCapabilityResources,
};
use ryframe_kernel::{AppError, AppResult};

impl ProductService {
    /// 在调用方持有的控制库事务中读取当前真正可用的能力版本，供配置包导出
    /// 生成只读依赖声明。部署不可用或仅保留 entitlement 的能力不会进入配置包。
    pub async fn enabled_capability_requirements_in_txn(
        &self,
        transaction: &dyn crate::ProductTransactionPort,
        tenant_id: &str,
    ) -> AppResult<Vec<CapabilityRequirement>> {
        let context =
            self.context_from_snapshot(transaction.current_tenant_product(tenant_id).await?)?;
        let mut requirements = context
            .capabilities
            .into_iter()
            .filter(|capability| capability.enabled)
            .map(|capability| {
                Ok(CapabilityRequirement {
                    code: capability.capability_code,
                    variant: capability
                        .variant_code
                        .ok_or_else(|| AppError::Config("已启用能力缺少有效 variant".into()))?,
                    schema_version: capability.schema_version.ok_or_else(|| {
                        AppError::Config("已启用能力缺少有效 schema_version".into())
                    })?,
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        requirements.sort();
        Ok(requirements)
    }

    /// 配置包进入目标租户前的强一致能力兼容校验。部署依赖、租户 entitlement、
    /// variant 与 schema 任一不匹配都拒绝，且绝不借配置包修改产品上下文。
    pub async fn ensure_capability_requirements_in_txn(
        &self,
        transaction: &dyn crate::ProductTransactionPort,
        tenant_id: &str,
        requirements: &[CapabilityRequirement],
    ) -> AppResult<()> {
        if requirements.is_empty() {
            return Ok(());
        }
        let context =
            self.context_from_snapshot(transaction.current_tenant_product(tenant_id).await?)?;
        let mut seen = BTreeSet::new();
        for requirement in requirements {
            if !seen.insert(requirement.code.as_str()) {
                return Err(AppError::Validation(format!(
                    "配置包重复声明能力 {}",
                    requirement.code
                )));
            }
            let descriptor = CAPABILITY_CATALOG
                .iter()
                .find(|descriptor| descriptor.code == requirement.code)
                .ok_or_else(|| {
                    AppError::CapabilityUnavailable(format!(
                        "配置包要求的能力 {} 未编译进当前部署",
                        requirement.code
                    ))
                })?;
            if !descriptor.variants.iter().any(|variant| {
                variant.code == requirement.variant
                    && variant.schema_version == requirement.schema_version
            }) {
                return Err(AppError::Validation(format!(
                    "配置包声明了能力 {} 不受支持的 variant/schema",
                    requirement.code
                )));
            }
            let current = context
                .capabilities
                .iter()
                .find(|capability| capability.capability_code == requirement.code)
                .ok_or_else(|| {
                    AppError::CapabilityUnavailable(format!(
                        "配置包要求的能力 {} 未编译进当前部署",
                        requirement.code
                    ))
                })?;
            if !current.deployment_enabled {
                return Err(AppError::CapabilityUnavailable(format!(
                    "当前部署不满足配置包能力 {} 的基础设施依赖",
                    requirement.code
                )));
            }
            if !current.entitled {
                return Err(AppError::TenantCapabilityDenied(format!(
                    "当前租户未开通配置包要求的能力 {}",
                    requirement.code
                )));
            }
            if current.variant_code.as_deref() != Some(requirement.variant.as_str())
                || current.schema_version != Some(requirement.schema_version)
            {
                return Err(AppError::TenantCapabilityDenied(format!(
                    "当前租户能力 {} 的 variant/schema 与配置包不兼容",
                    requirement.code
                )));
            }
        }
        Ok(())
    }

    pub async fn provisioning_resources(
        &self,
        version_id: i64,
    ) -> AppResult<ProvisioningCapabilityResources> {
        let target = self.published_target(version_id).await?;
        let enabled_codes = target
            .capabilities
            .iter()
            .map(|capability| capability.code.as_str())
            .collect::<BTreeSet<_>>();
        Ok(resources_for_enabled_codes(&enabled_codes))
    }

    /// 租户创建首事务内重新锁定并校验可分配版本，防止与 retire/套餐停用并发。
    pub async fn provisioning_resources_in_txn(
        &self,
        transaction: &dyn crate::ProductTransactionPort,
        version_id: i64,
    ) -> AppResult<ProvisioningCapabilityResources> {
        let version = transaction.lock_assignable_version(version_id).await?;
        if version.plan_status != super::PLAN_STATUS_ENABLED {
            return Err(AppError::Conflict("目标产品套餐已停用".into()));
        }
        if version.version_status != super::VERSION_PUBLISHED {
            return Err(AppError::Conflict("目标产品套餐版本已不再可分配".into()));
        }
        self.validate_publishable_capability_records(&version.capabilities)?;
        let enabled_codes = version
            .capabilities
            .iter()
            .map(|capability| capability.code.as_str())
            .collect::<BTreeSet<_>>();
        Ok(resources_for_enabled_codes(&enabled_codes))
    }

    /// 在目标数据面 fence 建立后，为 provisioning 租户同步其套餐能力资源。
    ///
    /// 调用方必须先锁定租户行；本方法随后按 plan -> version 锁序复验套餐，且整次
    /// 权限、菜单和默认管理员授权在一个控制库事务内完成，因此失败可安全重试。
    pub async fn sync_provisioning_resources_in_txn(
        &self,
        transaction: &dyn crate::ProductTransactionPort,
        tenant_id: &str,
        version_id: i64,
    ) -> AppResult<()> {
        let resources = self
            .provisioning_resources_in_txn(transaction, version_id)
            .await?;
        transaction
            .sync_capability_resources(tenant_id, &resources)
            .await
    }

    /// 角色授权写入前的 Capability 守卫。普通角色和超级管理员都不能
    /// 绕过产品授权或部署依赖。
    pub(crate) fn ensure_permission_codes_enabled(
        &self,
        snapshot: crate::TenantProductSnapshot,
        permission_codes: &[String],
    ) -> AppResult<()> {
        let requested = permission_codes
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let guarded = CAPABILITY_CATALOG
            .iter()
            .filter(|descriptor| {
                descriptor
                    .permission_codes
                    .iter()
                    .any(|code| requested.contains(code))
            })
            .collect::<Vec<_>>();
        if guarded.is_empty() {
            return Ok(());
        }
        let context = self.context_from_snapshot(snapshot)?;
        for descriptor in guarded {
            let capability = context
                .capabilities
                .iter()
                .find(|capability| capability.capability_code == descriptor.code)
                .ok_or_else(|| {
                    AppError::CapabilityUnavailable(format!(
                        "能力 {} 未编译进当前部署",
                        descriptor.code
                    ))
                })?;
            if !capability.deployment_enabled {
                return Err(AppError::CapabilityUnavailable(format!(
                    "当前部署不满足能力 {} 的基础设施依赖",
                    descriptor.code
                )));
            }
            if !capability.entitled {
                return Err(AppError::TenantCapabilityDenied(format!(
                    "当前租户未开通能力 {}，不能授予其权限",
                    descriptor.code
                )));
            }
        }
        Ok(())
    }

    /// 路由权限同步只能看到当前租户真正可用的 Capability 权限。
    pub(crate) fn filter_syncable_permission_codes(
        &self,
        snapshot: crate::TenantProductSnapshot,
        permission_codes: BTreeSet<String>,
    ) -> AppResult<BTreeSet<String>> {
        let context = self.context_from_snapshot(snapshot)?;
        let enabled_capabilities = context
            .capabilities
            .iter()
            .filter(|capability| capability.enabled)
            .map(|capability| capability.capability_code.as_str())
            .collect::<HashSet<_>>();
        Ok(permission_codes
            .into_iter()
            .filter(|permission_code| {
                CAPABILITY_CATALOG.iter().all(|descriptor| {
                    !descriptor
                        .permission_codes
                        .contains(&permission_code.as_str())
                        || enabled_capabilities.contains(descriptor.code)
                })
            })
            .collect())
    }
}

fn resources_for_enabled_codes(enabled_codes: &BTreeSet<&str>) -> ProvisioningCapabilityResources {
    let enabled = CAPABILITY_CATALOG
        .iter()
        .filter(|descriptor| enabled_codes.contains(descriptor.code))
        .collect::<Vec<_>>();
    ProvisioningCapabilityResources {
        enabled_route_keys: enabled
            .iter()
            .flat_map(|descriptor| descriptor.route_keys)
            .map(|value| (*value).to_owned())
            .collect(),
        enabled_permission_codes: enabled
            .iter()
            .flat_map(|descriptor| descriptor.permission_codes)
            .map(|value| (*value).to_owned())
            .collect(),
        managed_route_keys: CAPABILITY_CATALOG
            .iter()
            .flat_map(|descriptor| descriptor.route_keys)
            .map(|value| (*value).to_owned())
            .collect(),
        managed_permission_codes: CAPABILITY_CATALOG
            .iter()
            .flat_map(|descriptor| descriptor.permission_codes)
            .map(|value| (*value).to_owned())
            .collect(),
        default_admin_permissions: enabled
            .iter()
            .flat_map(|descriptor| descriptor.default_admin_permissions)
            .map(|value| (*value).to_owned())
            .collect(),
    }
}

pub(super) fn resources_for_change(
    current: &ProductContextVo,
    target: &ProductContextVo,
) -> ProvisioningCapabilityResources {
    let enabled_codes = target
        .capabilities
        .iter()
        .filter(|capability| capability.entitled)
        .map(|capability| capability.capability_code.as_str())
        .collect::<BTreeSet<_>>();
    let mut resources = resources_for_enabled_codes(&enabled_codes);
    let previously_enabled = current
        .capabilities
        .iter()
        .filter(|capability| capability.entitled)
        .map(|capability| capability.capability_code.as_str())
        .collect::<BTreeSet<_>>();
    resources.default_admin_permissions = CAPABILITY_CATALOG
        .iter()
        .filter(|descriptor| {
            enabled_codes.contains(descriptor.code) && !previously_enabled.contains(descriptor.code)
        })
        .flat_map(|descriptor| descriptor.default_admin_permissions)
        .map(|value| (*value).to_owned())
        .collect();
    resources
}
