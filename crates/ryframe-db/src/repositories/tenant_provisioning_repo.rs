use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult, utils::snowflake};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait,
};

use crate::entities::{
    config, dept, dict_data, dict_type, menu, permission, post, role, role_permission, tenant,
    user, user_role,
};

const TEMPLATE_TENANT_ID: &str = "system";

#[derive(Debug, Clone)]
pub struct ProvisionTenantCommand {
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_minute: i32,
    pub admin_username: String,
    pub admin_password_hash: String,
}

pub struct TenantProvisioningRepository;

impl TenantProvisioningRepository {
    pub async fn admin_username_exists(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        username: &str,
    ) -> AppResult<bool> {
        user::Entity::find()
            .filter(user::Column::TenantId.eq(tenant_id))
            .filter(user::Column::Username.eq(username))
            .one(db)
            .await
            .map(|user| user.is_some())
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn provision(
        &self,
        db: &DatabaseConnection,
        command: ProvisionTenantCommand,
    ) -> AppResult<tenant::Model> {
        let transaction = db
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let now = Utc::now();

        let system_menus = menu::Entity::find()
            .filter(menu::Column::TenantId.eq(TEMPLATE_TENANT_ID))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .order_by_asc(menu::Column::Id)
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_posts = post::Entity::find()
            .filter(post::Column::TenantId.eq(TEMPLATE_TENANT_ID))
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_configs = config::Entity::find()
            .filter(config::Column::TenantId.eq(TEMPLATE_TENANT_ID))
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_dict_types = dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(TEMPLATE_TENANT_ID))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let mut system_dict_data = dict_data::Entity::find()
            .filter(dict_data::Column::TenantId.eq(TEMPLATE_TENANT_ID))
            .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_depts = dept::Entity::find()
            .filter(dept::Column::TenantId.eq(TEMPLATE_TENANT_ID))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .order_by_asc(dept::Column::Id)
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

        retain_data_for_active_dict_types(&system_dict_types, &mut system_dict_data);
        let system_permissions = permission::Entity::find()
            .filter(permission::Column::TenantId.eq(TEMPLATE_TENANT_ID))
            .filter(permission::Column::Code.ne("*:*:*"))
            .filter(permission::Column::Code.not_like("tenant:%"))
            .order_by_asc(permission::Column::Id)
            .all(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

        let tenant_id = command.tenant_id;
        let tenant = tenant::ActiveModel {
            id: ActiveValue::Set(snowflake::try_next_snowflake_id()?),
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            name: ActiveValue::Set(command.name),
            domain: ActiveValue::Set(command.domain),
            status: ActiveValue::Set(tenant::Model::STATUS_NORMAL.to_string()),
            expire_at: ActiveValue::Set(command.expire_at),
            max_users: ActiveValue::Set(command.max_users),
            max_roles: ActiveValue::Set(command.max_roles),
            max_storage_mb: ActiveValue::Set(command.max_storage_mb),
            max_requests_per_min: ActiveValue::Set(command.max_requests_per_minute),
            session_version: ActiveValue::Set(1),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(&transaction)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        let admin_role_id = snowflake::try_next_snowflake_id()?;
        let user_role_id = snowflake::try_next_snowflake_id()?;
        let user_id = snowflake::try_next_snowflake_id()?;

        role::ActiveModel {
            id: ActiveValue::Set(admin_role_id),
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            name: ActiveValue::Set("租户管理员".into()),
            code: ActiveValue::Set("tenant_admin".into()),
            is_super: ActiveValue::Set(0),
            data_scope: ActiveValue::Set(role::Model::DATA_SCOPE_ALL.into()),
            status: ActiveValue::Set(role::Model::STATUS_NORMAL.into()),
            sort: ActiveValue::Set(1),
            remark: ActiveValue::Set(Some("创建租户时自动初始化".into())),
            del_flag: ActiveValue::Set(role::Model::DEL_FLAG_NORMAL.into()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(&transaction)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        role::ActiveModel {
            id: ActiveValue::Set(user_role_id),
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            name: ActiveValue::Set("租户普通用户".into()),
            code: ActiveValue::Set("tenant_user".into()),
            is_super: ActiveValue::Set(0),
            data_scope: ActiveValue::Set(role::Model::DATA_SCOPE_SELF.into()),
            status: ActiveValue::Set(role::Model::STATUS_NORMAL.into()),
            sort: ActiveValue::Set(0),
            remark: ActiveValue::Set(Some("租户初始化的只读角色".into())),
            del_flag: ActiveValue::Set(role::Model::DEL_FLAG_NORMAL.into()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(&transaction)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        user::ActiveModel {
            id: ActiveValue::Set(user_id),
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            username: ActiveValue::Set(command.admin_username),
            password_hash: ActiveValue::Set(command.admin_password_hash),
            nickname: ActiveValue::Set("租户管理员".into()),
            email: ActiveValue::Set(String::new()),
            phone: ActiveValue::Set(String::new()),
            avatar: ActiveValue::Set(None),
            status: ActiveValue::Set(user::Model::STATUS_NORMAL.into()),
            auth_version: ActiveValue::Set(0),
            dept_id: ActiveValue::Set(None),
            remark: ActiveValue::Set(None),
            login_ip: ActiveValue::Set(None),
            login_date: ActiveValue::Set(None),
            del_flag: ActiveValue::Set(user::Model::DEL_FLAG_NORMAL.into()),
            created_at: ActiveValue::Set(now),
            updated_at: ActiveValue::Set(now),
        }
        .insert(&transaction)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        user_role::ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            user_id: ActiveValue::Set(user_id),
            role_id: ActiveValue::Set(admin_role_id),
        }
        .insert(&transaction)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        let mut permission_ids = HashMap::new();
        let mut role_permissions = Vec::new();
        for source in system_permissions {
            let id = snowflake::try_next_snowflake_id()?;
            let parent_id = source
                .parent_id
                .and_then(|parent_id| permission_ids.get(&parent_id).copied());
            permission::ActiveModel {
                id: ActiveValue::Set(id),
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                name: ActiveValue::Set(source.name),
                code: ActiveValue::Set(source.code.clone()),
                parent_id: ActiveValue::Set(parent_id),
                perm_type: ActiveValue::Set(source.perm_type),
                icon: ActiveValue::Set(source.icon),
                sort: ActiveValue::Set(source.sort),
                status: ActiveValue::Set(source.status),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            }
            .insert(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

            role_permissions.push(role_permission::ActiveModel {
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                role_id: ActiveValue::Set(admin_role_id),
                perm_id: ActiveValue::Set(id),
            });
            if source.code.ends_with(":query")
                || source.code.ends_with(":list")
                || source.code.ends_with(":view")
            {
                role_permissions.push(role_permission::ActiveModel {
                    tenant_id: ActiveValue::Set(tenant_id.clone()),
                    role_id: ActiveValue::Set(user_role_id),
                    perm_id: ActiveValue::Set(id),
                });
            }
            permission_ids.insert(source.id, id);
        }
        if !role_permissions.is_empty() {
            role_permission::Entity::insert_many(role_permissions)
                .exec(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }

        let mut menu_ids = HashMap::new();
        for source in system_menus {
            let id = snowflake::try_next_snowflake_id()?;
            let parent_id = source
                .parent_id
                .and_then(|parent_id| menu_ids.get(&parent_id).copied());
            let perm_id = source
                .perm_id
                .and_then(|perm_id| permission_ids.get(&perm_id).copied());
            menu::ActiveModel {
                id: ActiveValue::Set(id),
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                name: ActiveValue::Set(source.name),
                parent_id: ActiveValue::Set(parent_id),
                menu_type: ActiveValue::Set(source.menu_type),
                perm_id: ActiveValue::Set(perm_id),
                route_key: ActiveValue::Set(source.route_key),
                icon: ActiveValue::Set(source.icon),
                sort: ActiveValue::Set(source.sort),
                visible: ActiveValue::Set(source.visible),
                status: ActiveValue::Set(source.status),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.del_flag),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            }
            .insert(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
            menu_ids.insert(source.id, id);
        }

        let posts = system_posts
            .into_iter()
            .map(|source| -> AppResult<_> {
                Ok(post::ActiveModel {
                    id: ActiveValue::Set(snowflake::try_next_snowflake_id()?),
                    tenant_id: ActiveValue::Set(tenant_id.clone()),
                    name: ActiveValue::Set(source.name),
                    code: ActiveValue::Set(source.code),
                    sort: ActiveValue::Set(source.sort),
                    status: ActiveValue::Set(source.status),
                    remark: ActiveValue::Set(source.remark),
                    del_flag: ActiveValue::Set(source.del_flag),
                    created_at: ActiveValue::Set(now),
                    updated_at: ActiveValue::Set(now),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        if !posts.is_empty() {
            post::Entity::insert_many(posts)
                .exec(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }

        let configs = system_configs
            .into_iter()
            .map(|source| -> AppResult<_> {
                Ok(config::ActiveModel {
                    id: ActiveValue::Set(snowflake::try_next_snowflake_id()?),
                    tenant_id: ActiveValue::Set(tenant_id.clone()),
                    name: ActiveValue::Set(source.name),
                    key: ActiveValue::Set(source.key),
                    value: ActiveValue::Set(source.value),
                    remark: ActiveValue::Set(source.remark),
                    del_flag: ActiveValue::Set(source.del_flag),
                    created_at: ActiveValue::Set(now),
                    updated_at: ActiveValue::Set(now),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        if !configs.is_empty() {
            config::Entity::insert_many(configs)
                .exec(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }

        let dict_types = system_dict_types
            .into_iter()
            .map(|source| -> AppResult<_> {
                Ok(dict_type::ActiveModel {
                    id: ActiveValue::Set(snowflake::try_next_snowflake_id()?),
                    tenant_id: ActiveValue::Set(tenant_id.clone()),
                    name: ActiveValue::Set(source.name),
                    code: ActiveValue::Set(source.code),
                    status: ActiveValue::Set(source.status),
                    remark: ActiveValue::Set(source.remark),
                    del_flag: ActiveValue::Set(source.del_flag),
                    created_at: ActiveValue::Set(now),
                    updated_at: ActiveValue::Set(now),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        if !dict_types.is_empty() {
            dict_type::Entity::insert_many(dict_types)
                .exec(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }

        let dict_data_models = system_dict_data
            .into_iter()
            .map(|source| -> AppResult<_> {
                Ok(dict_data::ActiveModel {
                    id: ActiveValue::Set(snowflake::try_next_snowflake_id()?),
                    tenant_id: ActiveValue::Set(tenant_id.clone()),
                    type_code: ActiveValue::Set(source.type_code),
                    label: ActiveValue::Set(source.label),
                    value: ActiveValue::Set(source.value),
                    sort: ActiveValue::Set(source.sort),
                    status: ActiveValue::Set(source.status),
                    css_class: ActiveValue::Set(source.css_class),
                    remark: ActiveValue::Set(source.remark),
                    del_flag: ActiveValue::Set(source.del_flag),
                    created_at: ActiveValue::Set(now),
                    updated_at: ActiveValue::Set(now),
                })
            })
            .collect::<AppResult<Vec<_>>>()?;
        if !dict_data_models.is_empty() {
            dict_data::Entity::insert_many(dict_data_models)
                .exec(&transaction)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        }

        let mut dept_ids: HashMap<i64, i64> = HashMap::new();
        for source in system_depts {
            let id = snowflake::try_next_snowflake_id()?;
            let parent_id = source
                .parent_id
                .and_then(|parent_id| dept_ids.get(&parent_id).copied());
            let ancestors = source
                .ancestors
                .split(',')
                .filter_map(|part| part.trim().parse::<i64>().ok())
                .filter_map(|old_id| dept_ids.get(&old_id).copied())
                .map(|new_id| new_id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            dept::ActiveModel {
                id: ActiveValue::Set(id),
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                name: ActiveValue::Set(source.name),
                parent_id: ActiveValue::Set(parent_id),
                ancestors: ActiveValue::Set(ancestors),
                sort: ActiveValue::Set(source.sort),
                status: ActiveValue::Set(source.status),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.del_flag),
                created_at: ActiveValue::Set(now),
                updated_at: ActiveValue::Set(now),
            }
            .insert(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
            dept_ids.insert(source.id, id);
        }

        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(tenant)
    }
}

/// A soft-deleted dictionary type must not make provisioning fail through its
/// composite foreign key, nor leave data reachable without a copied type.
fn retain_data_for_active_dict_types(
    dictionary_types: &[dict_type::Model],
    dictionary_data: &mut Vec<dict_data::Model>,
) {
    let active_codes: HashSet<&str> = dictionary_types
        .iter()
        .map(|dictionary| dictionary.code.as_str())
        .collect();
    dictionary_data.retain(|data| active_codes.contains(data.type_code.as_str()));
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::retain_data_for_active_dict_types;
    use crate::entities::{dict_data, dict_type};

    fn dictionary_type(code: &str) -> dict_type::Model {
        dict_type::Model {
            id: 1,
            tenant_id: "system".into(),
            name: code.into(),
            code: code.into(),
            status: dict_type::Model::STATUS_NORMAL.into(),
            remark: None,
            del_flag: dict_type::Model::DEL_FLAG_NORMAL.into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn dictionary_data(id: i64, type_code: &str) -> dict_data::Model {
        dict_data::Model {
            id,
            tenant_id: "system".into(),
            type_code: type_code.into(),
            label: id.to_string(),
            value: id.to_string(),
            sort: 0,
            status: dict_data::Model::STATUS_NORMAL.into(),
            css_class: None,
            remark: None,
            del_flag: dict_data::Model::DEL_FLAG_NORMAL.into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn provisioning_drops_data_whose_type_is_not_copied() {
        let types = vec![dictionary_type("active")];
        let mut data = vec![dictionary_data(1, "active"), dictionary_data(2, "deleted")];

        retain_data_for_active_dict_types(&types, &mut data);

        assert_eq!(data.len(), 1);
        assert_eq!(data[0].type_code, "active");
    }
}
