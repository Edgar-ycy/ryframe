use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult, utils::snowflake};
use ryframe_db::{
    TenantRepository,
    entities::{
        config, dept, dict_data, dict_type, menu, permission, post as post_entity, role,
        role_permission, tenant, user, user_role,
    },
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, TransactionTrait,
};

#[derive(Debug, Clone)]
pub struct CreateTenantParams {
    pub tenant_id: String,
    pub name: String,
    pub domain: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: Option<i32>,
    pub max_roles: Option<i32>,
    pub max_storage_mb: Option<i64>,
    pub max_requests_per_min: Option<i32>,
    pub admin_username: String,
    pub admin_password: String,
}

#[derive(Debug, Clone)]
pub struct UpdateTenantParams {
    pub name: String,
    pub domain: Option<String>,
    pub expire_at: Option<DateTime<Utc>>,
    pub max_users: i32,
    pub max_roles: i32,
    pub max_storage_mb: i64,
    pub max_requests_per_min: i32,
}

pub struct TenantServiceImpl {
    pub tenant_repo: TenantRepository,
}

impl TenantServiceImpl {
    pub async fn list(&self, db: &DatabaseConnection) -> AppResult<Vec<tenant::Model>> {
        tenant::Entity::find()
            .order_by_asc(tenant::Column::TenantId)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn create(
        &self,
        db: &DatabaseConnection,
        params: CreateTenantParams,
    ) -> AppResult<tenant::Model> {
        if self
            .tenant_repo
            .find_by_tenant_id(db, &params.tenant_id)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("租户标识已存在".into()));
        }
        if user::Entity::find()
            .filter(user::Column::TenantId.eq(&params.tenant_id))
            .filter(user::Column::Username.eq(&params.admin_username))
            .one(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .is_some()
        {
            return Err(AppError::Conflict("租户管理员用户名已存在".into()));
        }

        let txn = db
            .begin()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

        let system_menus = menu::Entity::find()
            .filter(menu::Column::TenantId.eq("system"))
            .order_by_asc(menu::Column::Id)
            .all(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_posts = post_entity::Entity::find()
            .filter(post_entity::Column::TenantId.eq("system"))
            .all(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_configs = config::Entity::find()
            .filter(config::Column::TenantId.eq("system"))
            .all(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_dict_types = dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq("system"))
            .all(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_dict_data = dict_data::Entity::find()
            .filter(dict_data::Column::TenantId.eq("system"))
            .all(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_depts = dept::Entity::find()
            .filter(dept::Column::TenantId.eq("system"))
            .order_by_asc(dept::Column::Id)
            .all(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let system_permissions = permission::Entity::find()
            .filter(permission::Column::TenantId.eq("system"))
            .filter(permission::Column::Code.ne("*:*:*"))
            .filter(permission::Column::Code.ne("tenant:manage"))
            .order_by_asc(permission::Column::Id)
            .all(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

        let tenant_id = params.tenant_id.clone();
        let model = tenant::ActiveModel {
            id: ActiveValue::Set(snowflake::next_snowflake_id()),
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            name: ActiveValue::Set(params.name),
            domain: ActiveValue::Set(params.domain),
            status: ActiveValue::Set(tenant::Model::STATUS_NORMAL.to_string()),
            expire_at: ActiveValue::Set(params.expire_at),
            max_users: ActiveValue::Set(params.max_users.unwrap_or(100)),
            max_roles: ActiveValue::Set(params.max_roles.unwrap_or(20)),
            max_storage_mb: ActiveValue::Set(params.max_storage_mb.unwrap_or(1024)),
            max_requests_per_min: ActiveValue::Set(params.max_requests_per_min.unwrap_or(1000)),
            session_version: ActiveValue::Set(1),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        let admin_role_id = snowflake::next_snowflake_id();
        let user_role_id = snowflake::next_snowflake_id();
        let user_id = snowflake::next_snowflake_id();

        role::ActiveModel {
            id: ActiveValue::Set(admin_role_id),
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            name: ActiveValue::Set("租户管理员".into()),
            code: ActiveValue::Set("tenant_admin".into()),
            data_scope: ActiveValue::Set(role::Model::DATA_SCOPE_ALL.into()),
            status: ActiveValue::Set(role::Model::STATUS_NORMAL.into()),
            sort: ActiveValue::Set(1),
            remark: ActiveValue::Set(Some("创建租户时自动初始化".into())),
            del_flag: ActiveValue::Set(role::Model::DEL_FLAG_NORMAL.into()),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        role::ActiveModel {
            id: ActiveValue::Set(user_role_id),
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            name: ActiveValue::Set("租户普通用户".into()),
            code: ActiveValue::Set("tenant_user".into()),
            data_scope: ActiveValue::Set(role::Model::DATA_SCOPE_SELF.into()),
            status: ActiveValue::Set(role::Model::STATUS_NORMAL.into()),
            sort: ActiveValue::Set(0),
            remark: ActiveValue::Set(Some("租户初始化的只读角色".into())),
            del_flag: ActiveValue::Set(role::Model::DEL_FLAG_NORMAL.into()),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        user::ActiveModel {
            id: ActiveValue::Set(user_id),
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            username: ActiveValue::Set(params.admin_username),
            password_hash: ActiveValue::Set(ryframe_auth::password::hash(&params.admin_password)?),
            nickname: ActiveValue::Set("租户管理员".into()),
            email: ActiveValue::Set(String::new()),
            phone: ActiveValue::Set(String::new()),
            avatar: ActiveValue::Set(None),
            status: ActiveValue::Set(user::Model::STATUS_NORMAL.into()),
            dept_id: ActiveValue::Set(None),
            remark: ActiveValue::Set(None),
            login_ip: ActiveValue::Set(None),
            login_date: ActiveValue::Set(None),
            del_flag: ActiveValue::Set(user::Model::DEL_FLAG_NORMAL.into()),
            ..Default::default()
        }
        .insert(&txn)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        user_role::ActiveModel {
            tenant_id: ActiveValue::Set(tenant_id.clone()),
            user_id: ActiveValue::Set(user_id),
            role_id: ActiveValue::Set(admin_role_id),
        }
        .insert(&txn)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        let mut menu_ids = HashMap::new();
        for source in system_menus {
            let id = snowflake::next_snowflake_id();
            let parent_id = source
                .parent_id
                .and_then(|parent_id| menu_ids.get(&parent_id).copied());
            menu::ActiveModel {
                id: ActiveValue::Set(id),
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                name: ActiveValue::Set(source.name),
                parent_id: ActiveValue::Set(parent_id),
                menu_type: ActiveValue::Set(source.menu_type),
                icon: ActiveValue::Set(source.icon),
                sort: ActiveValue::Set(source.sort),
                visible: ActiveValue::Set(source.visible),
                status: ActiveValue::Set(source.status),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.del_flag),
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
            menu_ids.insert(source.id, id);
        }

        let mut permission_ids = HashMap::new();
        for source in system_permissions {
            let id = snowflake::next_snowflake_id();
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
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;

            role_permission::ActiveModel {
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                role_id: ActiveValue::Set(admin_role_id),
                perm_id: ActiveValue::Set(id),
            }
            .insert(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
            if source.code.ends_with(":query")
                || source.code.ends_with(":list")
                || source.code.ends_with(":view")
            {
                role_permission::ActiveModel {
                    tenant_id: ActiveValue::Set(tenant_id.clone()),
                    role_id: ActiveValue::Set(user_role_id),
                    perm_id: ActiveValue::Set(id),
                }
                .insert(&txn)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
            }
            permission_ids.insert(source.id, id);
        }

        for source in system_posts {
            post_entity::ActiveModel {
                id: ActiveValue::Set(snowflake::next_snowflake_id()),
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                name: ActiveValue::Set(source.name),
                code: ActiveValue::Set(source.code),
                sort: ActiveValue::Set(source.sort),
                status: ActiveValue::Set(source.status),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.del_flag),
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        }

        for source in system_configs {
            config::ActiveModel {
                id: ActiveValue::Set(snowflake::next_snowflake_id()),
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                name: ActiveValue::Set(source.name),
                key: ActiveValue::Set(source.key),
                value: ActiveValue::Set(source.value),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.del_flag),
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        }

        for source in system_dict_types {
            dict_type::ActiveModel {
                id: ActiveValue::Set(snowflake::next_snowflake_id()),
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                name: ActiveValue::Set(source.name),
                code: ActiveValue::Set(source.code),
                status: ActiveValue::Set(source.status),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.del_flag),
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        }

        for source in system_dict_data {
            dict_data::ActiveModel {
                id: ActiveValue::Set(snowflake::next_snowflake_id()),
                tenant_id: ActiveValue::Set(tenant_id.clone()),
                type_code: ActiveValue::Set(source.type_code),
                label: ActiveValue::Set(source.label),
                value: ActiveValue::Set(source.value),
                sort: ActiveValue::Set(source.sort),
                status: ActiveValue::Set(source.status),
                css_class: ActiveValue::Set(source.css_class),
                remark: ActiveValue::Set(source.remark),
                del_flag: ActiveValue::Set(source.del_flag),
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        }

        let mut dept_ids: HashMap<i64, i64> = HashMap::new();
        for source in system_depts {
            let id = snowflake::next_snowflake_id();
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
                ..Default::default()
            }
            .insert(&txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
            dept_ids.insert(source.id, id);
        }

        txn.commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(model)
    }

    pub async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        params: UpdateTenantParams,
    ) -> AppResult<tenant::Model> {
        let current = self
            .tenant_repo
            .find_by_tenant_id(db, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let next_session_version = if params.expire_at != current.expire_at {
            current.session_version.saturating_add(1)
        } else {
            current.session_version
        };
        tenant::ActiveModel {
            id: ActiveValue::Unchanged(current.id),
            tenant_id: ActiveValue::Unchanged(current.tenant_id),
            name: ActiveValue::Set(params.name),
            domain: ActiveValue::Set(params.domain),
            status: ActiveValue::Unchanged(current.status),
            expire_at: ActiveValue::Set(params.expire_at),
            max_users: ActiveValue::Set(params.max_users),
            max_roles: ActiveValue::Set(params.max_roles),
            max_storage_mb: ActiveValue::Set(params.max_storage_mb),
            max_requests_per_min: ActiveValue::Set(params.max_requests_per_min),
            session_version: ActiveValue::Set(next_session_version),
            ..Default::default()
        }
        .update(db)
        .await
        .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn update_status(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        status: String,
    ) -> AppResult<()> {
        if tenant_id == "system" {
            return Err(AppError::Validation("不能停用 system 租户".into()));
        }
        if !matches!(status.as_str(), "0" | "1") {
            return Err(AppError::Validation("无效的租户状态".into()));
        }
        let result = tenant::Entity::update_many()
            .col_expr(
                tenant::Column::Status,
                sea_orm::sea_query::Expr::value(status),
            )
            .col_expr(
                tenant::Column::SessionVersion,
                sea_orm::sea_query::Expr::cust("session_version + 1"),
            )
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected == 0 {
            return Err(AppError::NotFound("租户不存在".into()));
        }
        Ok(())
    }
}
