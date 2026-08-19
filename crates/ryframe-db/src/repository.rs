use async_trait::async_trait;
use ryframe_kernel::{AppResult, PageResult, ValidatedPageQuery};
use sea_orm::DatabaseConnection;

/// 控制库实体使用的通用 SQL Repository 契约。
#[async_trait]
pub trait Repository<T, ID>: Send + Sync {
    /// 根据主键查询单条记录。
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: ID,
    ) -> AppResult<Option<T>>;

    /// 分页查询。
    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<T>>;

    /// 插入新记录并返回实体。
    async fn insert(&self, db: &DatabaseConnection, tenant_id: &str, entity: T) -> AppResult<T>;

    /// 更新记录并返回实体。
    async fn update(&self, db: &DatabaseConnection, tenant_id: &str, entity: T) -> AppResult<T>;

    /// 根据主键删除记录。
    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: ID) -> AppResult<()>;
}
