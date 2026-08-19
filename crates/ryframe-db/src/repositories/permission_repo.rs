use std::collections::HashSet;

use async_trait::async_trait;
use ryframe_adapters::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, sea_query::LockType,
};

use crate::entities::{menu, permission, role_permission, user_role};

pub struct PermissionRepository;

#[async_trait]
impl Repository<permission::Model, i64> for PermissionRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<permission::Model>> {
        permission::Entity::find_by_id(id)
            .filter(permission::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<permission::Model>> {
        crate::pagination::paginate(
            db,
            permission::Entity::find().filter(permission::Column::TenantId.eq(tenant_id)),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: permission::Model,
    ) -> AppResult<permission::Model> {
        insert_entity!(permission, db, tenant_id, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: permission::Model,
    ) -> AppResult<permission::Model> {
        update_entity!(permission, db, tenant_id, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        permission::Entity::delete_many()
            .filter(permission::Column::Id.eq(id))
            .filter(permission::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl PermissionRepository {
    pub async fn find_by_id_for_update(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<permission::Model>> {
        permission::Entity::find_by_id(id)
            .filter(permission::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn insert_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: permission::Model,
    ) -> AppResult<permission::Model> {
        if entity.tenant_id != tenant_id {
            return Err(AppError::Authorization("权限租户不匹配".into()));
        }
        permission::ActiveModel::from(entity)
            .insert(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn update_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: permission::Model,
    ) -> AppResult<permission::Model> {
        if entity.tenant_id != tenant_id {
            return Err(AppError::Authorization("权限租户不匹配".into()));
        }
        permission::ActiveModel::from(entity)
            .reset_all()
            .update(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn delete_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<()> {
        let result = permission::Entity::delete_many()
            .filter(permission::Column::Id.eq(id))
            .filter(permission::Column::TenantId.eq(tenant_id))
            .exec(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("权限不存在".into()));
        }
        Ok(())
    }

    /// 查询租户内指定 ID 的权限，用于批量存在性校验。
    pub async fn find_by_ids(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        perm_ids: &[i64],
    ) -> AppResult<Vec<permission::Model>> {
        if perm_ids.is_empty() {
            return Ok(Vec::new());
        }

        permission::Entity::find()
            .filter(permission::Column::Id.is_in(perm_ids.iter().copied()))
            .filter(permission::Column::TenantId.eq(tenant_id))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_affected_user_ids(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        perm_ids: &[i64],
    ) -> AppResult<Vec<i64>> {
        if perm_ids.is_empty() {
            return Ok(Vec::new());
        }
        let role_ids: HashSet<i64> = role_permission::Entity::find()
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .filter(role_permission::Column::PermId.is_in(perm_ids.iter().copied()))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .into_iter()
            .map(|row| row.role_id)
            .collect();
        if role_ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut user_ids: Vec<i64> = user_role::Entity::find()
            .filter(user_role::Column::TenantId.eq(tenant_id))
            .filter(user_role::Column::RoleId.is_in(role_ids))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .into_iter()
            .map(|row| row.user_id)
            .collect();
        user_ids.sort_unstable();
        user_ids.dedup();
        Ok(user_ids)
    }

    pub async fn is_referenced<C>(&self, db: &C, tenant_id: &str, perm_id: i64) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let role_reference = role_permission::Entity::find()
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .filter(role_permission::Column::PermId.eq(perm_id))
            .lock(LockType::Update)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .is_some();
        if role_reference {
            return Ok(true);
        }
        menu::Entity::find()
            .filter(menu::Column::TenantId.eq(tenant_id))
            .filter(menu::Column::DelFlag.eq(menu::Model::DEL_FLAG_NORMAL))
            .filter(menu::Column::PermId.eq(perm_id))
            .lock(LockType::Update)
            .one(db)
            .await
            .map(|row| row.is_some())
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_by_code(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<permission::Model>> {
        permission::Entity::find()
            .filter(permission::Column::Code.eq(code))
            .filter(permission::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 批量查询角色的权限码（去重）
    ///
    /// 返回所有角色拥有的权限实体列表，权限码已去重。
    pub async fn find_role_perms(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        role_ids: &[i64],
    ) -> AppResult<Vec<permission::Model>> {
        if role_ids.is_empty() {
            return Ok(vec![]);
        }

        let perm_ids: Vec<i64> = role_permission::Entity::find()
            .filter(role_permission::Column::RoleId.is_in(role_ids.iter().copied()))
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .into_iter()
            .map(|rp| rp.perm_id)
            .collect();

        if perm_ids.is_empty() {
            return Ok(vec![]);
        }

        permission::Entity::find()
            .filter(permission::Column::TenantId.eq(tenant_id))
            .filter(permission::Column::Id.is_in(perm_ids))
            .filter(permission::Column::Status.eq("1"))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 查询角色绑定的权限ID列表
    pub async fn find_role_perm_ids(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        role_id: i64,
    ) -> AppResult<Vec<i64>> {
        let ids = role_permission::Entity::find()
            .filter(role_permission::Column::RoleId.eq(role_id))
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .into_iter()
            .map(|rp| rp.perm_id)
            .collect();
        Ok(ids)
    }

    /// 在调用方事务内为角色替换权限关系（先删后插）。
    pub async fn assign_perms(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        role_id: i64,
        perm_ids: &[i64],
    ) -> AppResult<()> {
        // 清除现有权限
        role_permission::Entity::delete_many()
            .filter(role_permission::Column::RoleId.eq(role_id))
            .filter(role_permission::Column::TenantId.eq(tenant_id))
            .exec(transaction)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        if perm_ids.is_empty() {
            return Ok(());
        }

        let models: Vec<role_permission::ActiveModel> = perm_ids
            .iter()
            .map(|pid| role_permission::ActiveModel {
                tenant_id: sea_orm::ActiveValue::Set(tenant_id.to_owned()),
                role_id: sea_orm::ActiveValue::Set(role_id),
                perm_id: sea_orm::ActiveValue::Set(*pid),
            })
            .collect();

        role_permission::Entity::insert_many(models)
            .exec(transaction)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 查询所有权限
    pub async fn find_all(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
    ) -> AppResult<Vec<permission::Model>> {
        permission::Entity::find()
            .filter(permission::Column::TenantId.eq(tenant_id))
            .order_by_asc(permission::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
