use async_trait::async_trait;
use ryframe_common::AppResult;
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entities::{permission, role_permission};

pub struct PermissionRepository;

#[async_trait]
impl Repository<permission::Model, i64> for PermissionRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<permission::Model>> {
        permission::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<permission::Model>> {
        crate::pagination::paginate(db, permission::Entity::find(), &query).await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        entity: permission::Model,
    ) -> AppResult<permission::Model> {
        let active: permission::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        entity: permission::Model,
    ) -> AppResult<permission::Model> {
        let active: permission::ActiveModel = entity.into();
        active
            .update(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        permission::Entity::delete_by_id(id)
            .exec(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl PermissionRepository {
    /// 批量查询角色的权限码（去重）
    ///
    /// 返回所有角色拥有的权限实体列表，权限码已去重。
    pub async fn find_role_perms(
        &self,
        db: &DatabaseConnection,
        role_ids: &[i64],
    ) -> AppResult<Vec<permission::Model>> {
        if role_ids.is_empty() {
            return Ok(vec![]);
        }

        let perm_ids: Vec<i64> = role_permission::Entity::find()
            .filter(role_permission::Column::RoleId.is_in(role_ids.iter().copied()))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?
            .into_iter()
            .map(|rp| rp.perm_id)
            .collect();

        if perm_ids.is_empty() {
            return Ok(vec![]);
        }

        permission::Entity::find()
            .filter(permission::Column::Id.is_in(perm_ids))
            .filter(permission::Column::Status.eq(1))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }

    /// 查询角色绑定的权限ID列表
    pub async fn find_role_perm_ids(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
    ) -> AppResult<Vec<i64>> {
        let ids = role_permission::Entity::find()
            .filter(role_permission::Column::RoleId.eq(role_id))
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?
            .into_iter()
            .map(|rp| rp.perm_id)
            .collect();
        Ok(ids)
    }

    /// 为角色分配权限（先删后插）
    pub async fn assign_perms(
        &self,
        db: &DatabaseConnection,
        role_id: i64,
        perm_ids: &[i64],
    ) -> AppResult<()> {
        // 清除现有权限
        role_permission::Entity::delete_many()
            .filter(role_permission::Column::RoleId.eq(role_id))
            .exec(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;

        if perm_ids.is_empty() {
            return Ok(());
        }

        let models: Vec<role_permission::ActiveModel> = perm_ids
            .iter()
            .map(|pid| role_permission::ActiveModel {
                role_id: sea_orm::ActiveValue::Set(role_id),
                perm_id: sea_orm::ActiveValue::Set(*pid),
            })
            .collect();

        role_permission::Entity::insert_many(models)
            .exec(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))?;
        Ok(())
    }

    /// 查询所有权限
    pub async fn find_all(
        &self,
        db: &DatabaseConnection,
    ) -> AppResult<Vec<permission::Model>> {
        permission::Entity::find()
            .order_by_asc(permission::Column::Sort)
            .all(db)
            .await
            .map_err(|e| ryframe_common::AppError::Database(e.to_string()))
    }
}