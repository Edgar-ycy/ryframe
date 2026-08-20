use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::Utc;
use ryframe_db::entities::{menu, permission, role, role_permission};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseTransaction, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder,
};

use super::{
    CAPABILITY_CATALOG, CapabilityRequirement, ProductContextVo, ProductService,
    ProvisioningCapabilityResources,
};

const TEMPLATE_TENANT_ID: &str = "system";

impl ProductService {
    /// 在调用方持有的控制库事务中读取当前真正可用的能力版本，供配置包导出
    /// 生成只读依赖声明。部署不可用或仅保留 entitlement 的能力不会进入配置包。
    pub async fn enabled_capability_requirements_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
    ) -> AppResult<Vec<CapabilityRequirement>> {
        let bundle = self
            .repository
            .tenant_product(transaction, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let context = self.context_from_bundle(bundle)?;
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
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        requirements: &[CapabilityRequirement],
    ) -> AppResult<()> {
        if requirements.is_empty() {
            return Ok(());
        }
        let bundle = self
            .repository
            .tenant_product(transaction, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let context = self.context_from_bundle(bundle)?;
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
            .map(|capability| capability.capability_code.as_str())
            .collect::<BTreeSet<_>>();
        Ok(resources_for_enabled_codes(&enabled_codes))
    }

    /// 租户创建首事务内重新锁定并校验可分配版本，防止与 retire/套餐停用并发。
    pub async fn provisioning_resources_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        version_id: i64,
    ) -> AppResult<ProvisioningCapabilityResources> {
        let observed = self
            .repository
            .find_version_by_id(transaction, version_id)
            .await?
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))?;
        let plan = self
            .repository
            .lock_plan_by_id_in_txn(transaction, observed.plan.id)
            .await?;
        let version = self
            .repository
            .lock_version_by_id_in_txn(transaction, version_id)
            .await?;
        if plan.status != super::PLAN_STATUS_ENABLED {
            return Err(AppError::Conflict("目标产品套餐已停用".into()));
        }
        if version.status != super::VERSION_PUBLISHED {
            return Err(AppError::Conflict("目标产品套餐版本已不再可分配".into()));
        }
        let capabilities = self
            .repository
            .list_capabilities(transaction, version_id)
            .await?;
        self.validate_publishable_capabilities(&capabilities)?;
        let enabled_codes = capabilities
            .iter()
            .map(|capability| capability.capability_code.as_str())
            .collect::<BTreeSet<_>>();
        Ok(resources_for_enabled_codes(&enabled_codes))
    }

    /// 在目标数据面 fence 建立后，为 provisioning 租户同步其套餐能力资源。
    ///
    /// 调用方必须先锁定租户行；本方法随后按 plan -> version 锁序复验套餐，且整次
    /// 权限、菜单和默认管理员授权在一个控制库事务内完成，因此失败可安全重试。
    pub async fn sync_provisioning_resources_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        version_id: i64,
    ) -> AppResult<()> {
        let resources = self
            .provisioning_resources_in_txn(transaction, version_id)
            .await?;
        let permission_ids = sync_permissions(transaction, tenant_id, &resources).await?;
        sync_menus(transaction, tenant_id, &resources, &permission_ids).await?;
        assign_default_admin_permissions(
            transaction,
            tenant_id,
            &resources.default_admin_permissions,
            &permission_ids,
        )
        .await
    }

    /// 产品变更与租户初始化共享的 Capability 资源同步。
    ///
    /// 禁用时仅休眠权限/菜单，不删除角色关系；重新启用可恢复历史授权。
    /// 首次启用时从 system 租户的受控模板补齐资源，并仅向 tenant_admin
    /// 追加 descriptor 声明的默认权限。
    pub(super) async fn sync_capability_resources_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        current: &ProductContextVo,
        target: &ProductContextVo,
    ) -> AppResult<()> {
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
                enabled_codes.contains(descriptor.code)
                    && !previously_enabled.contains(descriptor.code)
            })
            .flat_map(|descriptor| descriptor.default_admin_permissions)
            .map(|value| (*value).to_owned())
            .collect();
        let permission_ids = sync_permissions(transaction, tenant_id, &resources).await?;
        sync_menus(transaction, tenant_id, &resources, &permission_ids).await?;
        assign_default_admin_permissions(
            transaction,
            tenant_id,
            &resources.default_admin_permissions,
            &permission_ids,
        )
        .await
    }

    /// 角色授权写入前的 Capability 守卫。普通角色和超级管理员都不能
    /// 绕过产品授权或部署依赖。
    pub async fn ensure_permission_codes_enabled_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
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
        let bundle = self
            .repository
            .tenant_product(transaction, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let context = self.context_from_bundle(bundle)?;
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
    pub async fn filter_syncable_permission_codes_in_txn(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        permission_codes: BTreeSet<String>,
    ) -> AppResult<BTreeSet<String>> {
        let bundle = self
            .repository
            .tenant_product(transaction, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let context = self.context_from_bundle(bundle)?;
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

async fn sync_permissions(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    resources: &ProvisioningCapabilityResources,
) -> AppResult<HashMap<String, i64>> {
    let managed = resources
        .managed_permission_codes
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let enabled = resources
        .enabled_permission_codes
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let system_permissions = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .order_by_asc(permission::Column::Id)
        .all(transaction)
        .await
        .map_err(database_error)?;
    let system_by_id = system_permissions
        .iter()
        .map(|permission| (permission.id, permission))
        .collect::<HashMap<_, _>>();
    let mut tenant_permissions = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(tenant_id))
        .order_by_asc(permission::Column::Id)
        .all(transaction)
        .await
        .map_err(database_error)?;
    let mut tenant_ids = tenant_permissions
        .iter()
        .map(|permission| (permission.code.clone(), permission.id))
        .collect::<HashMap<_, _>>();

    for source in system_permissions
        .iter()
        .filter(|source| managed.contains(source.code.as_str()))
    {
        let desired_status = if enabled.contains(source.code.as_str()) {
            "1"
        } else {
            "0"
        };
        if let Some(existing) = tenant_permissions
            .iter_mut()
            .find(|permission| permission.code == source.code)
        {
            if existing.status != desired_status {
                existing.status = desired_status.into();
                existing.updated_at = Utc::now();
                existing
                    .clone()
                    .into_active_model()
                    .reset_all()
                    .update(transaction)
                    .await
                    .map_err(database_error)?;
            }
            continue;
        }
        if desired_status == "0" {
            continue;
        }
        let parent_id = match source.parent_id {
            Some(source_parent_id) => {
                let parent_code = &system_by_id
                    .get(&source_parent_id)
                    .ok_or_else(|| AppError::Config("能力权限模板父级不存在".into()))?
                    .code;
                Some(*tenant_ids.get(parent_code).ok_or_else(|| {
                    AppError::Config(format!("租户 {tenant_id} 缺少能力权限父级 {parent_code}"))
                })?)
            }
            None => None,
        };
        let id = crate::next_id()?;
        permission::ActiveModel {
            id: Set(id),
            tenant_id: Set(tenant_id.to_owned()),
            name: Set(source.name.clone()),
            code: Set(source.code.clone()),
            parent_id: Set(parent_id),
            perm_type: Set(source.perm_type.clone()),
            icon: Set(source.icon.clone()),
            sort: Set(source.sort),
            status: Set("1".into()),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
        }
        .insert(transaction)
        .await
        .map_err(database_error)?;
        tenant_ids.insert(source.code.clone(), id);
    }
    Ok(tenant_ids)
}

async fn sync_menus(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    resources: &ProvisioningCapabilityResources,
    permission_ids: &HashMap<String, i64>,
) -> AppResult<()> {
    let managed = resources
        .managed_route_keys
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let enabled = resources
        .enabled_route_keys
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let system_menus = menu::Entity::find()
        .filter(menu::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .order_by_asc(menu::Column::Id)
        .all(transaction)
        .await
        .map_err(database_error)?;
    let system_by_id = system_menus
        .iter()
        .map(|menu| (menu.id, menu))
        .collect::<HashMap<_, _>>();
    let system_permission_codes = permission::Entity::find()
        .filter(permission::Column::TenantId.eq(TEMPLATE_TENANT_ID))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|permission| (permission.id, permission.code))
        .collect::<HashMap<_, _>>();
    let mut tenant_menus = menu::Entity::find()
        .filter(menu::Column::TenantId.eq(tenant_id))
        .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
        .order_by_asc(menu::Column::Id)
        .all(transaction)
        .await
        .map_err(database_error)?;
    let mut tenant_menu_ids = tenant_menus
        .iter()
        .filter_map(|menu| menu.route_key.clone().map(|route_key| (route_key, menu.id)))
        .collect::<HashMap<_, _>>();

    for source in system_menus.iter().filter(|source| {
        source
            .route_key
            .as_deref()
            .is_some_and(|route_key| managed.contains(route_key))
    }) {
        let route_key = source.route_key.as_deref().expect("filtered route key");
        let is_enabled = enabled.contains(route_key);
        if let Some(existing) = tenant_menus
            .iter_mut()
            .find(|menu| menu.route_key.as_deref() == Some(route_key))
        {
            let desired_status = if is_enabled { "1" } else { "0" };
            if existing.status != desired_status || existing.visible != is_enabled {
                existing.status = desired_status.into();
                existing.visible = is_enabled;
                existing.updated_at = Utc::now();
                existing
                    .clone()
                    .into_active_model()
                    .reset_all()
                    .update(transaction)
                    .await
                    .map_err(database_error)?;
            }
            continue;
        }
        if !is_enabled {
            continue;
        }
        let parent_id = match source.parent_id {
            Some(source_parent_id) => {
                let parent_route = system_by_id
                    .get(&source_parent_id)
                    .and_then(|parent| parent.route_key.as_deref())
                    .ok_or_else(|| AppError::Config("能力菜单父级缺少 route_key".into()))?;
                Some(*tenant_menu_ids.get(parent_route).ok_or_else(|| {
                    AppError::Config(format!("租户 {tenant_id} 缺少能力菜单父级 {parent_route}"))
                })?)
            }
            None => None,
        };
        let perm_id = match source.perm_id {
            Some(source_perm_id) => {
                let code = system_permission_codes
                    .get(&source_perm_id)
                    .ok_or_else(|| AppError::Config("能力菜单引用的模板权限不存在".into()))?;
                Some(*permission_ids.get(code).ok_or_else(|| {
                    AppError::Config(format!("租户 {tenant_id} 缺少能力菜单权限 {code}"))
                })?)
            }
            None => None,
        };
        let id = crate::next_id()?;
        menu::ActiveModel {
            id: Set(id),
            tenant_id: Set(tenant_id.to_owned()),
            name: Set(source.name.clone()),
            parent_id: Set(parent_id),
            menu_type: Set(source.menu_type.clone()),
            perm_id: Set(perm_id),
            route_key: Set(source.route_key.clone()),
            icon: Set(source.icon.clone()),
            sort: Set(source.sort),
            visible: Set(true),
            status: Set(menu::Model::STATUS_NORMAL.into()),
            remark: Set(source.remark.clone()),
            del_flag: Set(menu::Model::DEL_FLAG_NORMAL.into()),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
        }
        .insert(transaction)
        .await
        .map_err(database_error)?;
        tenant_menu_ids.insert(route_key.to_owned(), id);
    }
    Ok(())
}

async fn assign_default_admin_permissions(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    default_codes: &[String],
    permission_ids: &HashMap<String, i64>,
) -> AppResult<()> {
    if default_codes.is_empty() {
        return Ok(());
    }
    let admin = role::Entity::find()
        .filter(role::Column::TenantId.eq(tenant_id))
        .filter(role::Column::Code.eq("tenant_admin"))
        .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
        .one(transaction)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::Config(format!("租户 {tenant_id} 缺少 tenant_admin 角色")))?;
    let requested = default_codes
        .iter()
        .map(|code| {
            permission_ids.get(code).copied().ok_or_else(|| {
                AppError::Config(format!("租户 {tenant_id} 缺少默认能力权限 {code}"))
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let existing = role_permission::Entity::find()
        .filter(role_permission::Column::TenantId.eq(tenant_id))
        .filter(role_permission::Column::RoleId.eq(admin.id))
        .filter(role_permission::Column::PermId.is_in(requested.iter().copied()))
        .all(transaction)
        .await
        .map_err(database_error)?
        .into_iter()
        .map(|relation| relation.perm_id)
        .collect::<HashSet<_>>();
    let additions = requested
        .into_iter()
        .filter(|permission_id| !existing.contains(permission_id))
        .map(|permission_id| role_permission::ActiveModel {
            tenant_id: Set(tenant_id.to_owned()),
            role_id: Set(admin.id),
            perm_id: Set(permission_id),
        })
        .collect::<Vec<_>>();
    if !additions.is_empty() {
        role_permission::Entity::insert_many(additions)
            .exec(transaction)
            .await
            .map_err(database_error)?;
    }
    Ok(())
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
