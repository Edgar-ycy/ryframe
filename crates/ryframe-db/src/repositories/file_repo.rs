use std::sync::Arc;

use async_trait::async_trait;
use ryframe_common::{AppError, AppResult, utils::ObjectStorage};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder,
};

use crate::entities::sys_file;

/// 文件元数据 Repository
///
/// 始终使用主数据库（`sys_file` 表仅存在于 primary 数据源）。
/// 上层调用可通过 `#[datasource("primary")]` 注解确保路由到主库。
pub struct FileRepository;

#[async_trait]
impl Repository<sys_file::Model, i64> for FileRepository {
    async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<sys_file::Model>> {
        sys_file::Entity::find_by_id(id)
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn find_by_page(
        &self,
        db: &DatabaseConnection,
        query: PageQuery,
    ) -> AppResult<PageResult<sys_file::Model>> {
        let paginator = sys_file::Entity::find()
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .order_by_desc(sys_file::Column::CreatedAt);

        crate::pagination::paginate(db, paginator, &query).await
    }

    async fn insert(
        &self,
        db: &DatabaseConnection,
        entity: sys_file::Model,
    ) -> AppResult<sys_file::Model> {
        let active: sys_file::ActiveModel = entity.into();
        active
            .insert(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn update(
        &self,
        db: &DatabaseConnection,
        entity: sys_file::Model,
    ) -> AppResult<sys_file::Model> {
        let active: sys_file::ActiveModel = entity.into();
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    async fn delete(&self, db: &DatabaseConnection, id: i64) -> AppResult<()> {
        let active = sys_file::ActiveModel {
            id: ActiveValue::Unchanged(id),
            del_flag: ActiveValue::Set(sys_file::Model::DEL_FLAG_DELETED.to_string()),
            updated_at: ActiveValue::Set(chrono::Utc::now()),
            ..Default::default()
        };
        active
            .update(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        Ok(())
    }
}

impl FileRepository {
    /// 按 bucket 查询文件列表
    pub async fn find_by_bucket(
        &self,
        db: &DatabaseConnection,
        bucket: &str,
    ) -> AppResult<Vec<sys_file::Model>> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .order_by_desc(sys_file::Column::CreatedAt)
            .all(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    /// 将数据库中存储的相对路径解析为可公开访问的完整 URL
    ///
    /// - 若 `file_url` 已是完整 URL（以 "http" 开头），直接返回（旧数据兼容）
    /// - 否则根据存储后端动态拼接完整 URL
    pub fn resolve_public_url(
        storage: &Arc<dyn ObjectStorage>,
        model: &sys_file::Model,
    ) -> String {
        // 兼容旧数据：如果已经存储了完整 URL，直接返回
        if model.file_url.starts_with("http://") || model.file_url.starts_with("https://") {
            return model.file_url.clone();
        }

        // file_url 存储格式: "{bucket}/{object_key}"
        // 需要提取 object_key（跳过 bucket/ 前缀）
        let object_key = if let Some(idx) = model.file_url.find('/') {
            &model.file_url[idx + 1..]
        } else {
            &model.file_url
        };

        let public_url = storage.public_url(&model.bucket, object_key);
        if public_url.is_empty() || public_url == "/" {
            format!(
                "/api/v1/common/file/download?bucket={}&path={}",
                model.bucket, object_key
            )
        } else {
            public_url
        }
    }
}
