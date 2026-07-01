use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder,
};

use crate::entities::{role, user_role};

pub struct RoleRepository;

#[async_trait]
impl Repository<role::Model, i64> for RoleRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<role::Model>> {
        role::Entity::find_by_id(id)
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .filter(role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .one(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<role::Model>> {
        crate::pagination::paginate(
            db,
            role::Entity::find().filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL)),
            &query,
        )
        .await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: role::Model) -> AppResult<role::Model> {
        insert_entity!(role, db, entity)
    }

    async fn update(&self, db: &DatabaseConnection, entity: role::Model) -> AppResult<role::Model> {
        update_entity!(role, db, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        soft_delete_entity!(role, db, id)
    }
}

impl RoleRepository {
    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<role::Model>> {
        let mut select = role::Entity::find()
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .filter(role::Column::TenantId.eq(ryframe_core::current_tenant_id()));

        if let Some(n) = name.filter(|n| !n.is_empty()) {
            select = select.filter(role::Column::Name.like(format!("%{}%", n)));
        }
        if let Some(c) = code.filter(|c| !c.is_empty()) {
            select = select.filter(role::Column::Code.like(format!("%{}%", c)));
        }
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            select = select.filter(role::Column::Status.eq(s));
        }

        select = select.order_by_asc(role::Column::Sort);
        crate::pagination::paginate(db, select, &query).await
    }

    /// 批量删除角色
    pub async fn delete_many(&self, db: &DatabaseConnection, ids: &[i64]) -> AppResult<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let result = role::Entity::update_many()
            .col_expr(
                role::Column::DelFlag,
                sea_orm::sea_query::Expr::value(role::Model::DEL_FLAG_DELETED),
            )
            .col_expr(
                role::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(role::Column::Id.is_in(ids.to_vec()))
            .filter(role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(result.rows_affected)
    }

    /// 查询用户拥有的角色列表
    pub async fn find_user_roles(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
    ) -> AppResult<Vec<role::Model>> {
        let role_ids: Vec<i64> = user_role::Entity::find()
            .filter(user_role::Column::UserId.eq(user_id))
            .filter(user_role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?
            .into_iter()
            .map(|ur| ur.role_id)
            .collect();

        if role_ids.is_empty() {
            return Ok(vec![]);
        }

        role::Entity::find()
            .filter(role::Column::Id.is_in(role_ids))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .filter(role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .filter(role::Column::Status.eq(role::Model::STATUS_NORMAL))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    /// 查询用户拥有的角色列表（包含停用角色，用于危险操作保护）
    pub async fn find_user_roles_all_status(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
    ) -> AppResult<Vec<role::Model>> {
        let role_ids: Vec<i64> = user_role::Entity::find()
            .filter(user_role::Column::UserId.eq(user_id))
            .filter(user_role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?
            .into_iter()
            .map(|ur| ur.role_id)
            .collect();

        if role_ids.is_empty() {
            return Ok(vec![]);
        }

        role::Entity::find()
            .filter(role::Column::Id.is_in(role_ids))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .filter(role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    /// 查询拥有任意指定角色的用户ID列表
    pub async fn find_user_ids_by_role_ids(
        &self,
        db: &DatabaseConnection,
        role_ids: &[i64],
    ) -> AppResult<Vec<i64>> {
        if role_ids.is_empty() {
            return Ok(vec![]);
        }

        let mut user_ids: Vec<i64> = user_role::Entity::find()
            .filter(user_role::Column::RoleId.is_in(role_ids.to_vec()))
            .filter(user_role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?
            .into_iter()
            .map(|ur| ur.user_id)
            .collect();
        user_ids.sort_unstable();
        user_ids.dedup();
        Ok(user_ids)
    }

    /// 清除用户全部角色关联
    pub async fn clear_user_roles(&self, db: &DatabaseConnection, user_id: i64) -> AppResult<()> {
        user_role::Entity::delete_many()
            .filter(user_role::Column::UserId.eq(user_id))
            .filter(user_role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .exec(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 在事务中为用户分配角色（先删后插）
    pub async fn assign_roles_in_txn(
        &self,
        txn: &DatabaseTransaction,
        user_id: i64,
        role_ids: &[i64],
    ) -> AppResult<()> {
        // 清除旧关联
        user_role::Entity::delete_many()
            .filter(user_role::Column::UserId.eq(user_id))
            .filter(user_role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .exec(txn)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;

        // 插入新关联
        if !role_ids.is_empty() {
            let models: Vec<user_role::ActiveModel> = role_ids
                .iter()
                .map(|rid| user_role::ActiveModel {
                    tenant_id: sea_orm::ActiveValue::Set(ryframe_core::current_tenant_id()),
                    user_id: sea_orm::ActiveValue::Set(user_id),
                    role_id: sea_orm::ActiveValue::Set(*rid),
                })
                .collect();

            user_role::Entity::insert_many(models)
                .exec(txn)
                .await
                .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        }
        Ok(())
    }

    /// 为用户分配角色（先删后插）
    pub async fn assign_roles(
        &self,
        db: &DatabaseConnection,
        user_id: i64,
        role_ids: &[i64],
    ) -> AppResult<()> {
        self.clear_user_roles(db, user_id).await?;

        let models: Vec<user_role::ActiveModel> = role_ids
            .iter()
            .map(|rid| user_role::ActiveModel {
                tenant_id: sea_orm::ActiveValue::Set(ryframe_core::current_tenant_id()),
                user_id: sea_orm::ActiveValue::Set(user_id),
                role_id: sea_orm::ActiveValue::Set(*rid),
            })
            .collect();

        user_role::Entity::insert_many(models)
            .exec(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 按角色编码查找
    pub async fn find_by_code(
        &self,
        db: &DatabaseConnection,
        code: &str,
    ) -> AppResult<Option<role::Model>> {
        role::Entity::find()
            .filter(role::Column::Code.eq(code))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .filter(role::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .one(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    /// 查询角色关联的自定义数据权限部门ID列表
    pub async fn find_role_dept_ids(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
    ) -> AppResult<Vec<i64>> {
        use crate::entities::role_dept;

        let ids = role_dept::Entity::find()
            .filter(role_dept::Column::RoleId.eq(role_id))
            .filter(role_dept::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?
            .into_iter()
            .map(|rd| rd.dept_id)
            .collect();
        Ok(ids)
    }

    /// 查询多个角色的所有自定义部门ID（合并去重）
    pub async fn find_roles_dept_ids(
        &self,
        db: &DatabaseConnection,
        role_ids: &[i64],
    ) -> AppResult<Vec<i64>> {
        use crate::entities::role_dept;

        if role_ids.is_empty() {
            return Ok(vec![]);
        }

        let ids = role_dept::Entity::find()
            .filter(role_dept::Column::RoleId.is_in(role_ids.to_vec()))
            .filter(role_dept::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?
            .into_iter()
            .map(|rd| rd.dept_id)
            .collect::<Vec<i64>>();
        let mut unique = ids;
        unique.sort();
        unique.dedup();
        Ok(unique)
    }

    /// 设置角色的数据权限（先删后插 sys_role_dept）
    pub async fn assign_data_scope_depts(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        dept_ids: &[i64],
    ) -> AppResult<()> {
        use sea_orm::{ActiveModelTrait, ActiveValue, TransactionTrait};

        use crate::entities::role_dept;

        let txn = db
            .begin()
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;

        // 删除旧关联
        role_dept::Entity::delete_many()
            .filter(role_dept::Column::RoleId.eq(role_id))
            .filter(role_dept::Column::TenantId.eq(ryframe_core::current_tenant_id()))
            .exec(&txn)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;

        // 插入新关联
        for dept_id in dept_ids {
            let rd = role_dept::ActiveModel {
                tenant_id: ActiveValue::Set(ryframe_core::current_tenant_id()),
                role_id: ActiveValue::Set(role_id),
                dept_id: ActiveValue::Set(*dept_id),
            };
            rd.insert(&txn)
                .await
                .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        }

        txn.commit()
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 更新角色的数据范围字段
    pub async fn update_data_scope(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        data_scope: &str,
    ) -> AppResult<()> {
        use sea_orm::ActiveValue;

        let role = self
            .find_by_id(db, role_id)
            .await?
            .ok_or_else(|| ryframe_common::AppError::NotFound("角色不存在".into()))?;

        let mut active: role::ActiveModel = role.into();
        active.data_scope = ActiveValue::Set(data_scope.to_string());
        active.updated_at = ActiveValue::Set(chrono::Utc::now());
        active
            .update(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// Atomically update the role scope and replace its custom departments.
    pub async fn replace_data_scope(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        data_scope: &str,
        dept_ids: &[i64],
    ) -> AppResult<()> {
        use crate::entities::role_dept;
        use sea_orm::{ActiveModelTrait, ActiveValue, TransactionTrait};

        let tenant_id = ryframe_core::current_tenant_id();
        let txn = db
            .begin()
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        let model = role::Entity::find_by_id(role_id)
            .filter(role::Column::TenantId.eq(&tenant_id))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .one(&txn)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?
            .ok_or_else(|| ryframe_common::AppError::NotFound("角色不存在".into()))?;

        let mut active: role::ActiveModel = model.into();
        active.data_scope = ActiveValue::Set(data_scope.to_string());
        active.updated_at = ActiveValue::Set(chrono::Utc::now());
        active
            .update(&txn)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;

        role_dept::Entity::delete_many()
            .filter(role_dept::Column::RoleId.eq(role_id))
            .filter(role_dept::Column::TenantId.eq(&tenant_id))
            .exec(&txn)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        if data_scope == role::Model::DATA_SCOPE_CUSTOM {
            for dept_id in dept_ids {
                role_dept::ActiveModel {
                    tenant_id: ActiveValue::Set(tenant_id.clone()),
                    role_id: ActiveValue::Set(role_id),
                    dept_id: ActiveValue::Set(*dept_id),
                }
                .insert(&txn)
                .await
                .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
            }
        }
        txn.commit()
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }
}
