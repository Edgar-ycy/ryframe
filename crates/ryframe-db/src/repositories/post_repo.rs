use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entities::post;

pub struct PostRepository;

#[async_trait]
impl Repository<post::Model, i64> for PostRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<post::Model>> {
        post::Entity::find_by_id(id).one(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<post::Model>> {
        crate::pagination::paginate(db, post::Entity::find(), &query).await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: post::Model) -> AppResult<post::Model> {
        let active: post::ActiveModel = entity.into();
        active.insert(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: post::Model) -> AppResult<post::Model> {
        let active: post::ActiveModel = entity.into();
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        post::Entity::delete_by_id(id).exec(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl PostRepository {
    /// 按岗位编码查找
    pub async fn find_by_code(&self, db: &DatabaseConnection, code: &str) -> AppResult<Option<post::Model>> {
        post::Entity::find()
            .filter(post::Column::Code.eq(code))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
