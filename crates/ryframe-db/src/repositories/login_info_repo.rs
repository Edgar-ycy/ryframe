use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::login_info;

pub struct LoginInfoRepository;

#[async_trait]
impl Repository<login_info::Model, i64> for LoginInfoRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<login_info::Model>> {
        login_info::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<login_info::Model>> {
        crate::pagination::paginate(
            db,
            login_info::Entity::find().order_by_desc(login_info::Column::LoginTime),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        entity: login_info::Model,
    ) -> AppResult<login_info::Model> {
        let active: login_info::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(
        &self,
        _db: &DatabaseConnection,
        _entity: login_info::Model,
    ) -> AppResult<login_info::Model> {
        Err(AppError::Internal("登录日志不支持修改".into()))
    }

    async fn delete(&self, _db: &DatabaseConnection, _id: i64) -> AppResult<()> {
        Err(AppError::Internal("登录日志不支持单条删除".into()))
    }
}

impl LoginInfoRepository {
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
        user_name: Option<&str>,
        status: Option<String>,
        begin_time: Option<DateTime<Utc>>,
        end_time: Option<DateTime<Utc>>,
    ) -> AppResult<PageResult<login_info::Model>> {
        let mut select = login_info::Entity::find();
        if let Some(name) = user_name {
            select = select.filter(login_info::Column::UserName.contains(name));
        }
        if let Some(s) = status {
            select = select.filter(login_info::Column::Status.eq(s));
        }
        if let Some(begin) = begin_time {
            select = select.filter(login_info::Column::LoginTime.gte(begin));
        }
        if let Some(end) = end_time {
            select = select.filter(login_info::Column::LoginTime.lte(end));
        }
        select = select.order_by_desc(login_info::Column::LoginTime);
        crate::pagination::paginate(db, select, &query).await
    }

    pub async fn clean_all(&self, db: &DatabaseConnection) -> AppResult<u64> {
        login_info::Entity::delete_many()
            .exec(db)
            .await
            .map(|r| r.rows_affected)
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
