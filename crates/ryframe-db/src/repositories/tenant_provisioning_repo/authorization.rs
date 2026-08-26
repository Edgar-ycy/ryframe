use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ryframe_application::ports::tenants::{
    TenantAuthorizationTemplate, TenantMenuTemplate, TenantPermissionTemplate,
    TenantProvisioningIdentity,
};
use ryframe_kernel::AppResult;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseTransaction, EntityTrait};

use super::TenantProvisioningRepository;
use crate::{
    DbResultExt,
    entities::{menu, permission, role_permission},
};

impl TenantProvisioningRepository {
    pub async fn copy_authorization_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        identity: &TenantProvisioningIdentity,
        template: TenantAuthorizationTemplate,
    ) -> AppResult<()> {
        let now = identity.provisioned_at;
        let permission_ids =
            copy_permissions(transaction, identity, template.permissions, now).await?;
        copy_menus(
            transaction,
            &identity.tenant_id,
            template.menus,
            &permission_ids,
            now,
        )
        .await
    }
}

async fn copy_permissions(
    transaction: &DatabaseTransaction,
    identity: &TenantProvisioningIdentity,
    permissions: Vec<TenantPermissionTemplate>,
    now: DateTime<Utc>,
) -> AppResult<HashMap<i64, i64>> {
    let mut permission_ids = HashMap::new();
    let mut grants = Vec::new();
    for source in permissions {
        let id = crate::next_id()?;
        let parent_id = source
            .parent_source_id
            .and_then(|parent_id| permission_ids.get(&parent_id).copied());
        permission::ActiveModel {
            id: ActiveValue::Set(id),
            tenant_id: ActiveValue::Set(identity.tenant_id.clone()),
            name: ActiveValue::Set(source.name),
            code: ActiveValue::Set(source.code),
            parent_id: ActiveValue::Set(parent_id),
            perm_type: ActiveValue::Set(source.permission_type),
            icon: ActiveValue::Set(source.icon),
            sort: ActiveValue::Set(source.sort),
            status: ActiveValue::Set(source.status),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(transaction)
        .await
        .db()?;
        if source.assign_admin {
            grants.push(role_grant(&identity.tenant_id, identity.admin_role_id, id));
        }
        if source.assign_user {
            grants.push(role_grant(&identity.tenant_id, identity.user_role_id, id));
        }
        permission_ids.insert(source.source_id, id);
    }
    if !grants.is_empty() {
        role_permission::Entity::insert_many(grants)
            .exec(transaction)
            .await
            .db()?;
    }
    Ok(permission_ids)
}

fn role_grant(tenant_id: &str, role_id: i64, permission_id: i64) -> role_permission::ActiveModel {
    role_permission::ActiveModel {
        tenant_id: ActiveValue::Set(tenant_id.to_owned()),
        role_id: ActiveValue::Set(role_id),
        perm_id: ActiveValue::Set(permission_id),
    }
}

async fn copy_menus(
    transaction: &DatabaseTransaction,
    tenant_id: &str,
    menus: Vec<TenantMenuTemplate>,
    permission_ids: &HashMap<i64, i64>,
    now: DateTime<Utc>,
) -> AppResult<()> {
    let mut menu_ids = HashMap::new();
    for source in menus {
        let id = crate::next_id()?;
        let parent_id = source
            .parent_source_id
            .and_then(|parent_id| menu_ids.get(&parent_id).copied());
        let permission_id = source
            .permission_source_id
            .and_then(|permission_id| permission_ids.get(&permission_id).copied());
        menu::ActiveModel {
            id: ActiveValue::Set(id),
            tenant_id: ActiveValue::Set(tenant_id.to_owned()),
            name: ActiveValue::Set(source.name),
            parent_id: ActiveValue::Set(parent_id),
            menu_type: ActiveValue::Set(source.menu_type),
            perm_id: ActiveValue::Set(permission_id),
            route_key: ActiveValue::Set(source.route_key),
            icon: ActiveValue::Set(source.icon),
            sort: ActiveValue::Set(source.sort),
            visible: ActiveValue::Set(source.visible),
            status: ActiveValue::Set(source.status),
            remark: ActiveValue::Set(source.remark),
            del_flag: ActiveValue::Set(source.delete_flag),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(transaction)
        .await
        .db()?;
        menu_ids.insert(source.source_id, id);
    }
    Ok(())
}
