use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::job_log;

pub struct JobLogRepository;

#[async_trait]
impl Repository<job_log::Model, i64> for JobLogRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<job_log::Model>> {
        job_log::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<job_log::Model>> {
        crate::pagination::paginate(
            db,
            job_log::Entity::find().order_by_desc(job_log::Column::StartTime),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        entity: job_log::Model,
    ) -> AppResult<job_log::Model> {
        let active: job_log::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(
        &self,
        _db: &DatabaseConnection,
        _entity: job_log::Model,
    ) -> AppResult<job_log::Model> {
        Err(AppError::Internal("任务日志不支持修改".into()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        job_log::Entity::delete_by_id(id)
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl JobLogRepository {
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        job_name: Option<&str>,
        status: Option<String>,
        begin: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
    ) -> AppResult<PageResult<job_log::Model>> {
        let mut select = job_log::Entity::find();
        if let Some(name) = job_name {
            select = select.filter(job_log::Column::JobName.eq(name));
        }
        if let Some(s) = status {
            select = select.filter(job_log::Column::Status.eq(s));
        }
        if let Some(b) = begin {
            select = select.filter(job_log::Column::StartTime.gte(b));
        }
        if let Some(e) = end {
            select = select.filter(job_log::Column::StartTime.lte(e));
        }
        select = select.order_by_desc(job_log::Column::StartTime);
        crate::pagination::paginate(db, select, &query).await
    }

    pub async fn clean_all(&self, db: &DatabaseConnection) -> AppResult<u64> {
        job_log::Entity::delete_many()
            .exec(db)
            .await
            .map(|r| r.rows_affected)
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn clean_before(&self, db: &DatabaseConnection, days: i64) -> AppResult<u64> {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        job_log::Entity::delete_many()
            .filter(job_log::Column::StartTime.lt(cutoff))
            .exec(db)
            .await
            .map(|r| r.rows_affected)
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
