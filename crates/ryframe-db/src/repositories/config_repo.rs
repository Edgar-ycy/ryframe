use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};

use crate::entities::config;

pub struct ConfigRepository;

#[async_trait]
impl Repository<config::Model, i64> for ConfigRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<config::Model>> {
        config::Entity::find_by_id(id).filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL)).one(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<config::Model>> {
        crate::pagination::paginate(db, config::Entity::find().filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL)), &query).await
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
        let active = config::ActiveModel {
            id: ActiveValue::Unchanged(id),
            del_flag: ActiveValue::Set(config::Model::DEL_FLAG_DELETED.to_string()),
            updated_at: ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl ConfigRepository {
    /// 按 key 查询配置
    pub async fn find_by_key(&self, db: &DatabaseConnection, key: &str) -> AppResult<Option<config::Model>> {
        config::Entity::find()
            .filter(config::Column::Key.eq(key))
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
