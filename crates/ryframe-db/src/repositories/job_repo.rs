use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};

use crate::entities::job;

pub struct JobRepository;

#[async_trait]
impl Repository<job::Model, i64> for JobRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<job::Model>> {
        job::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<job::Model>> {
        crate::pagination::paginate(
            db,
            job::Entity::find().order_by_asc(job::Column::CreateTime),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        entity: job::Model,
    ) -> AppResult<job::Model> {
        let active: job::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        entity: job::Model,
    ) -> AppResult<job::Model> {
        let active: job::ActiveModel = entity.into();
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        job::Entity::delete_by_id(id)
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl JobRepository {
    pub async fn find_by_name(
        &self,
        db: &DatabaseConnection,
        name: &str,
    ) -> AppResult<Option<job::Model>> {
        job::Entity::find()
            .filter(job::Column::Name.eq(name))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_all_enabled(
        &self,
        db: &DatabaseConnection,
    ) -> AppResult<Vec<job::Model>> {
        job::Entity::find()
            .filter(job::Column::Status.eq(job::Model::STATUS_NORMAL))
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn update_status(
        &self,
        db: &DatabaseConnection,
        id: i64,
        status: String,
    ) -> AppResult<()> {
        let active = job::ActiveModel {
            id: sea_orm::ActiveValue::Unchanged(id),
            status: sea_orm::ActiveValue::Set(status),
            update_time: sea_orm::ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}
