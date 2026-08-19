use async_trait::async_trait;
use ryframe_adapters::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect,
};

use crate::entities::post;

pub struct PostRepository;

#[derive(Debug, Default)]
pub struct PostFilter<'a> {
    pub name: Option<&'a str>,
    pub code: Option<&'a str>,
    pub status: Option<&'a str>,
}

#[async_trait]
impl Repository<post::Model, i64> for PostRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<post::Model>> {
        post::Entity::find_by_id(id)
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .filter(post::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<post::Model>> {
        crate::pagination::paginate(
            db,
            post::Entity::find()
                .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
                .filter(post::Column::TenantId.eq(tenant_id)),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: post::Model,
    ) -> AppResult<post::Model> {
        insert_entity!(post, db, tenant_id, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: post::Model,
    ) -> AppResult<post::Model> {
        update_entity!(post, db, tenant_id, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        soft_delete_entity!(post, db, tenant_id, id)
    }
}

impl PostRepository {
    pub async fn insert_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: post::Model,
    ) -> AppResult<post::Model> {
        insert_entity!(post, transaction, tenant_id, entity)
    }

    pub async fn update_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: post::Model,
    ) -> AppResult<post::Model> {
        update_entity!(post, transaction, tenant_id, entity)
    }

    pub async fn delete_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<()> {
        soft_delete_entity!(post, transaction, tenant_id, id)
    }

    /// 按主键递增游标读取岗位导出批次。
    pub async fn find_for_export_after_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        filter: &PostFilter<'_>,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<post::Model>> {
        let mut select = post::Entity::find()
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .filter(post::Column::TenantId.eq(tenant_id));
        if let Some(value) = filter.name.filter(|value| !value.is_empty()) {
            select = select.filter(post::Column::Name.like(format!("%{value}%")));
        }
        if let Some(value) = filter.code.filter(|value| !value.is_empty()) {
            select = select.filter(post::Column::Code.like(format!("%{value}%")));
        }
        if let Some(value) = filter.status.filter(|value| !value.is_empty()) {
            select = select.filter(post::Column::Status.eq(value));
        }
        if let Some(id) = after_id {
            select = select.filter(post::Column::Id.gt(id));
        }
        select
            .order_by_asc(post::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 按岗位编码查找
    pub async fn find_by_code(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<post::Model>> {
        post::Entity::find()
            .filter(post::Column::TenantId.eq(tenant_id))
            .filter(post::Column::Code.eq(code))
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 带搜索条件的分页查询
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<post::Model>> {
        let mut select = post::Entity::find()
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .filter(post::Column::TenantId.eq(tenant_id));
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            select = select.filter(post::Column::Name.like(format!("%{}%", n)));
        }
        if let Some(c) = code.filter(|c| !c.is_empty()) {
            select = select.filter(post::Column::Code.like(format!("%{}%", c)));
        }
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            select = select.filter(post::Column::Status.eq(s));
        }
        select = select.order_by_asc(post::Column::Sort);
        crate::pagination::paginate(db, select, &query).await
    }
}
