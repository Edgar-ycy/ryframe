use std::collections::{HashMap, HashSet};

use chrono::Utc;
use ryframe_application::ports::product::ProvisioningCapabilityResources;
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder,
};

use crate::entities::{menu, permission, role, role_permission};

use super::database_error;

const TEMPLATE_TENANT_ID: &str = "system";

pub(crate) async fn sync_persisted_capability_resources(
    transaction: &sea_orm::DatabaseTransaction,
    tenant_id: &str,
    resources: &ProvisioningCapabilityResources,
) -> AppResult<()> {
    let permission_ids = sync_permissions(transaction, tenant_id, resources).await?;
    sync_menus(transaction, tenant_id, resources, &permission_ids).await?;
    assign_default_admin_permissions(
        transaction,
        tenant_id,
        &resources.default_admin_permissions,
        &permission_ids,
    )
    .await
}

async fn sync_permissions(
    transaction: &sea_orm::DatabaseTransaction,
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
        let id = ryframe_application::next_id()?;
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
    transaction: &sea_orm::DatabaseTransaction,
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
        let id = ryframe_application::next_id()?;
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
    transaction: &sea_orm::DatabaseTransaction,
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
