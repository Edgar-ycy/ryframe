use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
};

use crate::entities::sys_file;

/// 文件元数据 Repository
///
/// 始终使用主数据库（`sys_file` 表仅存在于 primary 数据源）。
/// 上层调用时应显式传入主库连接。
pub struct FileRepository;

#[async_trait]
impl Repository<sys_file::Model, i64> for FileRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<sys_file::Model>> {
        sys_file::Entity::find_by_id(id)
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        query: PageQuery,
    ) -> AppResult<PageResult<sys_file::Model>> {
        let paginator = sys_file::Entity::find()
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .order_by_desc(sys_file::Column::CreatedAt);

        crate::pagination::paginate(db, paginator, &query).await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: sys_file::Model,
    ) -> AppResult<sys_file::Model> {
        insert_entity!(sys_file, db, tenant_id, entity)
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        entity: sys_file::Model,
    ) -> AppResult<sys_file::Model> {
        update_entity!(sys_file, db, tenant_id, entity)
    }

    async fn delete(&self, db: &DatabaseConnection, tenant_id: &str, id: i64) -> AppResult<()> {
        soft_delete_entity!(sys_file, db, tenant_id, id)
    }
}

impl FileRepository {
    /// 按 bucket 查询文件列表
    pub async fn find_by_bucket(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        bucket: &str,
    ) -> AppResult<Vec<sys_file::Model>> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .order_by_desc(sys_file::Column::CreatedAt)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 将数据库中存储的相对路径解析为可公开访问的完整 URL
    ///
    /// 根据存储后端和数据库中的相对对象路径拼接完整 URL。
    pub async fn find_by_md5(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        bucket: &str,
        file_md5: &str,
    ) -> AppResult<Option<sys_file::Model>> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::FileMd5.eq(file_md5))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_by_storage_path(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        bucket: &str,
        storage_path: &str,
    ) -> AppResult<Option<sys_file::Model>> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::StoragePath.eq(storage_path))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
