use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entities::{dict_data, dict_type};

pub struct DictTypeRepository;

#[async_trait]
impl Repository<dict_type::Model, i64> for DictTypeRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<dict_type::Model>> {
        dict_type::Entity::find_by_id(id).one(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<dict_type::Model>> {
        crate::pagination::paginate(db, dict_type::Entity::find(), &query).await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: dict_type::Model) -> AppResult<dict_type::Model> {
        let active: dict_type::ActiveModel = entity.into();
        active.insert(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: dict_type::Model) -> AppResult<dict_type::Model> {
        let active: dict_type::ActiveModel = entity.into();
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        dict_type::Entity::delete_by_id(id).exec(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl DictTypeRepository {
    /// 查找所有启用的字典类型
    pub async fn find_all(&self, db: &DatabaseConnection) -> AppResult<Vec<dict_type::Model>> {
        dict_type::Entity::find()
            .filter(dict_type::Column::Status.eq(dict_type::Model::STATUS_NORMAL))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 按编码查找字典类型
    pub async fn find_by_code(&self, db: &DatabaseConnection, code: &str) -> AppResult<Option<dict_type::Model>> {
        dict_type::Entity::find()
            .filter(dict_type::Column::Code.eq(code))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}

pub struct DictDataRepository;

#[async_trait]
impl Repository<dict_data::Model, i64> for DictDataRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<dict_data::Model>> {
        dict_data::Entity::find_by_id(id).one(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(&self, db: &DatabaseConnection, query: PageQuery) -> AppResult<PageResult<dict_data::Model>> {
        crate::pagination::paginate(db, dict_data::Entity::find(), &query).await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: dict_data::Model) -> AppResult<dict_data::Model> {
        let active: dict_data::ActiveModel = entity.into();
        active.insert(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(&self, db: &DatabaseConnection, entity: dict_data::Model) -> AppResult<dict_data::Model> {
        let active: dict_data::ActiveModel = entity.into();
        active.update(db).await.map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        dict_data::Entity::delete_by_id(id).exec(db).await.map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl DictDataRepository {
    /// 按字典类型编码获取字典数据
    pub async fn find_by_type_code(&self, db: &DatabaseConnection, type_code: &str) -> AppResult<Vec<dict_data::Model>> {
        dict_data::Entity::find()
            .filter(dict_data::Column::TypeCode.eq(type_code))
            .filter(dict_data::Column::Status.eq(dict_data::Model::STATUS_NORMAL))
            .order_by_asc(dict_data::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
