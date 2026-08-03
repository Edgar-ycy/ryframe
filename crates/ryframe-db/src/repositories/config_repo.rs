use async_trait::async_trait;
use ryframe_core::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Expr, LockType},
};

use crate::entities::config;

pub struct ConfigRepository;

#[derive(Debug, Default)]
pub struct ConfigFilter<'a> {
    pub name: Option<&'a str>,
    pub key: Option<&'a str>,
}

#[async_trait]
impl Repository<config::Model, i64> for ConfigRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<config::Model>> {
        config::Entity::find_by_id(id)
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .filter(config::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<config::Model>> {
        crate::pagination::paginate(
            db,
            config::Entity::find()
                .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
                .filter(config::Column::TenantId.eq(tenant_id)),
            &query,
        )
        .await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: config::Model,
    ) -> AppResult<config::Model> {
        insert_entity!(config, db, tenant_id, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: config::Model,
    ) -> AppResult<config::Model> {
        use sea_orm::sea_query::Expr;
        let now = chrono::Utc::now();
        // 对 auto_increment=false 的 MySQL 主键使用显式更新，避免依赖
        // ActiveModel::update() 在不同 SeaORM 2.0 版本间的执行差异。
        config::Entity::update_many()
            .col_expr(config::Column::Name, Expr::value(entity.name.clone()))
            .col_expr(config::Column::Key, Expr::value(entity.key.clone()))
            .col_expr(config::Column::Value, Expr::value(entity.value.clone()))
            .col_expr(config::Column::Remark, Expr::value(entity.remark.clone()))
            .col_expr(config::Column::UpdatedAt, Expr::value(now))
            .filter(config::Column::Id.eq(entity.id))
            .filter(config::Column::TenantId.eq(tenant_id))
            .exec(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        // 重新查询返回最新数据
        config::Entity::find_by_id(entity.id)
            .filter(config::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        soft_delete_entity!(config, db, tenant_id, id)
    }
}

impl ConfigRepository {
    pub async fn find_by_id_for_update(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<config::Model>> {
        config::Entity::find_by_id(id)
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .filter(config::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn insert_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: config::Model,
    ) -> AppResult<config::Model> {
        if entity.tenant_id != tenant_id {
            return Err(AppError::Authorization("参数配置租户不匹配".into()));
        }
        config::ActiveModel::from(entity)
            .insert(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn update_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        entity: config::Model,
    ) -> AppResult<config::Model> {
        if entity.tenant_id != tenant_id {
            return Err(AppError::Authorization("参数配置租户不匹配".into()));
        }
        let id = entity.id;
        let result = config::Entity::update_many()
            .col_expr(config::Column::Name, Expr::value(entity.name))
            .col_expr(config::Column::Key, Expr::value(entity.key))
            .col_expr(config::Column::Value, Expr::value(entity.value))
            .col_expr(config::Column::Remark, Expr::value(entity.remark))
            .col_expr(config::Column::UpdatedAt, Expr::value(entity.updated_at))
            .filter(config::Column::Id.eq(id))
            .filter(config::Column::TenantId.eq(tenant_id))
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .exec(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("参数配置不存在".into()));
        }
        config::Entity::find_by_id(id)
            .filter(config::Column::TenantId.eq(tenant_id))
            .one(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::NotFound("参数配置不存在".into()))
    }

    pub async fn delete_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<()> {
        let result = config::Entity::update_many()
            .col_expr(
                config::Column::DelFlag,
                Expr::value(config::Model::DEL_FLAG_DELETED),
            )
            .col_expr(config::Column::UpdatedAt, Expr::value(chrono::Utc::now()))
            .filter(config::Column::Id.eq(id))
            .filter(config::Column::TenantId.eq(tenant_id))
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .exec(transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected != 1 {
            return Err(AppError::NotFound("参数配置不存在".into()));
        }
        Ok(())
    }

    /// 按主键递增游标读取参数配置导出批次。
    pub async fn find_for_export_after_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        filter: &ConfigFilter<'_>,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<config::Model>> {
        let mut select = config::Entity::find()
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .filter(config::Column::TenantId.eq(tenant_id));
        if let Some(name) = filter.name.filter(|value| !value.is_empty()) {
            select = select.filter(config::Column::Name.contains(name));
        }
        if let Some(key) = filter.key.filter(|value| !value.is_empty()) {
            select = select.filter(config::Column::Key.contains(key));
        }
        if let Some(id) = after_id {
            select = select.filter(config::Column::Id.gt(id));
        }
        select
            .order_by_asc(config::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &ValidatedPageQuery,
        filter: &ConfigFilter<'_>,
    ) -> AppResult<PageResult<config::Model>> {
        let mut select = config::Entity::find()
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .filter(config::Column::TenantId.eq(tenant_id));
        if let Some(name) = filter.name.filter(|value| !value.is_empty()) {
            select = select.filter(config::Column::Name.contains(name));
        }
        if let Some(key) = filter.key.filter(|value| !value.is_empty()) {
            select = select.filter(config::Column::Key.contains(key));
        }
        crate::pagination::paginate(db, select.order_by_desc(config::Column::CreatedAt), query)
            .await
    }

    /// 按 key 查询配置
    pub async fn find_by_key(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        key: &str,
    ) -> AppResult<Option<config::Model>> {
        config::Entity::find()
            .filter(config::Column::Key.eq(key))
            .filter(config::Column::TenantId.eq(tenant_id))
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
