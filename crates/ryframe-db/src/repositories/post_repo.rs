use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::post;

pub struct PostRepository;
const DEFAULT_TENANT_ID: &str = "system";

#[async_trait]
impl Repository<post::Model, i64> for PostRepository {
    async fn find_by_id(&self, db: &DatabaseConnection, id: i64) -> AppResult<Option<post::Model>> {
        post::Entity::find_by_id(id)
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .filter(post::Column::TenantId.eq(DEFAULT_TENANT_ID))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<post::Model>> {
        crate::pagination::paginate(
            db,
            post::Entity::find()
                .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
                .filter(post::Column::TenantId.eq(DEFAULT_TENANT_ID)),
            &query,
        )
        .await
    }

    async fn insert(&self, db: &DatabaseConnection, entity: post::Model) -> AppResult<post::Model> {
        insert_entity!(post, db, entity)
    }

    async fn update(&self, db: &DatabaseConnection, entity: post::Model) -> AppResult<post::Model> {
        update_entity!(post, db, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        soft_delete_entity!(post, db, id)
    }
}

impl PostRepository {
    /// 按岗位编码查找
    pub async fn find_by_code(
        &self,
        db: &DatabaseConnection,
        code: &str,
    ) -> AppResult<Option<post::Model>> {
        post::Entity::find()
            .filter(post::Column::TenantId.eq(DEFAULT_TENANT_ID))
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
        query: PageQuery,
        name: Option<&str>,
        code: Option<&str>,
        status: Option<&str>,
    ) -> AppResult<PageResult<post::Model>> {
        let mut select = post::Entity::find()
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .filter(post::Column::TenantId.eq(DEFAULT_TENANT_ID));
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

    /// 查询所有岗位（用于导出）
    pub async fn find_all(&self, db: &DatabaseConnection) -> AppResult<Vec<post::Model>> {
        post::Entity::find()
            .filter(post::Column::TenantId.eq(DEFAULT_TENANT_ID))
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .order_by_asc(post::Column::Sort)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
