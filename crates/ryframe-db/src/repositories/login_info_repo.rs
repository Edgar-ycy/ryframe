use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult, DataScopeContext};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::login_info;

pub struct LoginInfoRepository;

pub struct LoginInfoFilter<'a> {
    pub user_name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub begin_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

#[async_trait]
impl Repository<login_info::Model, i64> for LoginInfoRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<login_info::Model>> {
        login_info::Entity::find_by_id(id)
            .filter(login_info::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: PageQuery,
    ) -> AppResult<PageResult<login_info::Model>> {
        crate::pagination::paginate(
            db,
            login_info::Entity::find()
                .filter(login_info::Column::TenantId.eq(tenant_id))
                .order_by_desc(login_info::Column::LoginTime),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: login_info::Model,
    ) -> AppResult<login_info::Model> {
        insert_entity!(login_info, db, tenant_id, entity)
    }

    async fn update(
        &self,
        _db: &DatabaseConnection,
        _tenant_id: &str,
        _entity: login_info::Model,
    ) -> AppResult<login_info::Model> {
        Err(AppError::Internal("登录日志不支持修改".into()))
    }

    async fn delete(&self, _db: &DatabaseConnection, _tenant_id: &str, _id: i64) -> AppResult<()> {
        Err(AppError::Internal("登录日志不支持单条删除".into()))
    }
}

impl LoginInfoRepository {
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &PageQuery,
        filter: LoginInfoFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<login_info::Model>> {
        let mut select =
            login_info::Entity::find().filter(login_info::Column::TenantId.eq(tenant_id));
        if let Some(name) = filter.user_name.filter(|n| !n.is_empty()) {
            select = select.filter(login_info::Column::UserName.contains(name));
        }
        if let Some(s) = filter.status.filter(|s| !s.is_empty()) {
            select = select.filter(login_info::Column::Status.eq(s));
        }
        if let Some(begin) = filter.begin_time {
            select = select.filter(login_info::Column::LoginTime.gte(begin));
        }
        if let Some(end) = filter.end_time {
            select = select.filter(login_info::Column::LoginTime.lte(end));
        }
        if let Some(condition) = crate::data_scope::owner_username_condition(
            login_info::Column::UserName,
            tenant_id,
            scope_ctx,
        ) {
            select = select.filter(condition);
        }
        select = select.order_by_desc(login_info::Column::LoginTime);
        crate::pagination::paginate(db, select, query).await
    }

    pub async fn clean_all(&self, db: &DatabaseConnection, tenant_id: &str) -> AppResult<u64> {
        login_info::Entity::delete_many()
            .filter(login_info::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map(|r| r.rows_affected)
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
