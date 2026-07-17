use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ryframe_common::{AppError, AppResult, DataScopeContext};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::oper_log;

pub struct OperLogRepository;

pub struct OperLogFilter<'a> {
    pub oper_name: Option<&'a str>,
    pub status: Option<&'a str>,
    pub begin_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
}

#[async_trait]
impl Repository<oper_log::Model, i64> for OperLogRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<oper_log::Model>> {
        oper_log::Entity::find_by_id(id)
            .filter(oper_log::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: PageQuery,
    ) -> AppResult<PageResult<oper_log::Model>> {
        crate::pagination::paginate(
            db,
            oper_log::Entity::find()
                .filter(oper_log::Column::TenantId.eq(tenant_id))
                .order_by_desc(oper_log::Column::OperTime),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: oper_log::Model,
    ) -> AppResult<oper_log::Model> {
        insert_entity!(oper_log, db, tenant_id, entity)
    }

    async fn update(
        &self,
        _db: &DatabaseConnection,
        _tenant_id: &str,
        _entity: oper_log::Model,
    ) -> AppResult<oper_log::Model> {
        Err(AppError::Internal("操作日志不支持修改".into()))
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        oper_log::Entity::delete_many()
            .filter(oper_log::Column::Id.eq(id))
            .filter(oper_log::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl OperLogRepository {
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &PageQuery,
        filter: OperLogFilter<'_>,
        scope_ctx: &DataScopeContext,
    ) -> AppResult<PageResult<oper_log::Model>> {
        let mut select = oper_log::Entity::find().filter(oper_log::Column::TenantId.eq(tenant_id));
        if let Some(name) = filter.oper_name.filter(|n| !n.is_empty()) {
            select = select.filter(oper_log::Column::OperName.contains(name));
        }
        if let Some(s) = filter.status.filter(|s| !s.is_empty()) {
            select = select.filter(oper_log::Column::Status.eq(s));
        }
        if let Some(begin) = filter.begin_time {
            select = select.filter(oper_log::Column::OperTime.gte(begin));
        }
        if let Some(end) = filter.end_time {
            select = select.filter(oper_log::Column::OperTime.lte(end));
        }
        if let Some(condition) = crate::data_scope::owner_username_condition(
            oper_log::Column::OperName,
            tenant_id,
            scope_ctx,
        ) {
            select = select.filter(condition);
        }
        select = select.order_by_desc(oper_log::Column::OperTime);
        crate::pagination::paginate(db, select, query).await
    }

    pub async fn clean_all(&self, db: &DatabaseConnection, tenant_id: &str) -> AppResult<u64> {
        oper_log::Entity::delete_many()
            .filter(oper_log::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map(|r| r.rows_affected)
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
