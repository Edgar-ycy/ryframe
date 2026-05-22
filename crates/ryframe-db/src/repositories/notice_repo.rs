use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait};

use crate::entities::notice;

pub struct NoticeRepository;

#[async_trait]
impl Repository<notice::Model, i64> for NoticeRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<notice::Model>> {
        notice::Entity::find_by_id(id).one(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<notice::Model>> {
        crate::pagination::paginate(db, notice::Entity::find(), &query).await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: notice::Model) -> AppResult<notice::Model> {
        let active: notice::ActiveModel = entity.into();
        active.insert(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: notice::Model) -> AppResult<notice::Model> {
        let active: notice::ActiveModel = entity.into();
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        notice::Entity::delete_by_id(id).exec(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}
