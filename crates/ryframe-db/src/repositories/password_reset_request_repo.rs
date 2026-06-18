use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::password_reset_request;

pub struct PasswordResetRequestRepository;

#[async_trait]
impl Repository<password_reset_request::Model, i64> for PasswordResetRequestRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<password_reset_request::Model>> {
        password_reset_request::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<password_reset_request::Model>> {
        crate::pagination::paginate(
            db,
            password_reset_request::Entity::find()
                .order_by_desc(password_reset_request::Column::CreatedAt),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        entity: password_reset_request::Model,
    ) -> AppResult<password_reset_request::Model> {
        insert_entity!(password_reset_request, db, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        entity: password_reset_request::Model,
    ) -> AppResult<password_reset_request::Model> {
        update_entity!(password_reset_request, db, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        password_reset_request::Entity::delete_by_id(id)
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl PasswordResetRequestRepository {
    pub async fn find_pending_by_target(
        &self,
        db: &DatabaseConnection,
        target_user_id: i64,
    ) -> AppResult<Vec<password_reset_request::Model>> {
        password_reset_request::Entity::find()
            .filter(password_reset_request::Column::TargetUserId.eq(target_user_id))
            .filter(
                password_reset_request::Column::Status
                    .eq(password_reset_request::Model::STATUS_PENDING),
            )
            .order_by_desc(password_reset_request::Column::CreatedAt)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
