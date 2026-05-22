use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entities::config;

pub struct ConfigRepository;

#[async_trait]
impl Repository<config::Model, i64> for ConfigRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<config::Model>> {
        config::Entity::find_by_id(id).one(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<config::Model>> {
        crate::pagination::paginate(db, config::Entity::find(), &query).await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: config::Model) -> AppResult<config::Model> {
        let active: config::ActiveModel = entity.into();
        active.insert(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: config::Model) -> AppResult<config::Model> {
        let active: config::ActiveModel = entity.into();
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        config::Entity::delete_by_id(id).exec(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl ConfigRepository {
    /// 按 key 查询配置
    pub async fn find_by_key(&self, db: &DatabaseConnection, key: &str) -> AppResult<Option<config::Model>> {
        config::Entity::find()
            .filter(config::Column::Key.eq(key))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
