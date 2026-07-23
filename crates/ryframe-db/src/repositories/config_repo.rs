use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
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
        query: PageQuery,
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
    pub async fn find_by_page_filtered(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: &PageQuery,
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
