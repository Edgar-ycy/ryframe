use async_trait::async_trait;
use ryframe_common::{AppError, AppResult};
use ryframe_core::repository::{PageQuery, PageResult, Repository};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Select, Statement,
    sea_query::LockType,
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
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
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
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
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
    /// Read the primary database's UTC clock so every application node makes
    /// lease and expiry decisions against the same authority.
    pub async fn database_utc_now<C>(&self, db: &C) -> AppResult<chrono::DateTime<chrono::Utc>>
    where
        C: ConnectionTrait + ?Sized,
    {
        let row = db
            .query_one_raw(Statement::from_string(
                db.get_database_backend(),
                "SELECT UTC_TIMESTAMP(6) AS db_now".to_owned(),
            ))
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .ok_or_else(|| AppError::Database("database clock query returned no row".into()))?;
        let now: chrono::NaiveDateTime = row
            .try_get("", "db_now")
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(chrono::DateTime::from_naive_utc_and_offset(
            now,
            chrono::Utc,
        ))
    }

    pub async fn insert_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        entity: sys_file::Model,
    ) -> AppResult<sys_file::Model> {
        insert_entity!(sys_file, txn, tenant_id, entity)
    }

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
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
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
        Self::find_by_md5_query(tenant_id, bucket, file_md5)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_by_md5_any_status_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        bucket: &str,
        file_md5: &str,
    ) -> AppResult<Option<sys_file::Model>> {
        Self::find_by_md5_any_status_query(tenant_id, bucket, file_md5)
            .lock(LockType::Update)
            .one(txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    fn find_by_md5_query(
        tenant_id: &str,
        bucket: &str,
        file_md5: &str,
    ) -> Select<sys_file::Entity> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::FileMd5.eq(file_md5))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
    }

    fn find_by_md5_any_status_query(
        tenant_id: &str,
        bucket: &str,
        file_md5: &str,
    ) -> Select<sys_file::Entity> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::FileMd5.eq(file_md5))
            .filter(Self::active_or_reserved_condition())
            .filter(sys_file::Column::TenantId.eq(tenant_id))
    }

    fn active_or_reserved_condition() -> Condition {
        Condition::any()
            .add(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .add(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
    }

    pub async fn find_by_id_any_status(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<sys_file::Model>> {
        sys_file::Entity::find_by_id(id)
            .filter(Self::active_or_reserved_condition())
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn mark_ready(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        reservation_token: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        let result = sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::UploadStatus,
                sea_orm::sea_query::Expr::value(sys_file::Model::UPLOAD_STATUS_READY),
            )
            .col_expr(
                sys_file::Column::ReservationToken,
                sea_orm::sea_query::Expr::value(Option::<String>::None),
            )
            .col_expr(
                sys_file::Column::ReservationExpiresAt,
                sea_orm::sea_query::Expr::value(Option::<chrono::DateTime<chrono::Utc>>::None),
            )
            .col_expr(
                sys_file::Column::DelFlag,
                sea_orm::sea_query::Expr::value(sys_file::Model::DEL_FLAG_NORMAL),
            )
            .col_expr(
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(updated_at),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
            .filter(sys_file::Column::ReservationToken.eq(reservation_token))
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(result.rows_affected == 1)
    }

    /// Extend an active upload lease using an ownership-token compare-and-set.
    pub async fn renew_pending_reservation(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        reservation_token: &str,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::ReservationExpiresAt,
                sea_orm::sea_query::Expr::value(expires_at),
            )
            .col_expr(
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
            .filter(sys_file::Column::ReservationToken.eq(reservation_token))
            .exec(db)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn begin_cleanup(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        reservation_token: &str,
        cleanup_after: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        let result = sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::UploadStatus,
                sea_orm::sea_query::Expr::value(sys_file::Model::UPLOAD_STATUS_CLEANUP),
            )
            .col_expr(
                sys_file::Column::ReservationExpiresAt,
                sea_orm::sea_query::Expr::value(cleanup_after),
            )
            .col_expr(
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(chrono::Utc::now()),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
            .filter(
                Condition::any()
                    .add(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
                    .add(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP)),
            )
            .filter(sys_file::Column::ReservationToken.eq(reservation_token))
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(result.rows_affected == 1)
    }

    pub async fn find_expired_reservations(
        &self,
        db: &DatabaseConnection,
        now: chrono::DateTime<chrono::Utc>,
        limit: u64,
    ) -> AppResult<Vec<sys_file::Model>> {
        sys_file::Entity::find()
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
            .filter(
                Condition::any()
                    .add(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
                    .add(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP)),
            )
            .filter(sys_file::Column::ReservationExpiresAt.lte(now))
            .order_by_asc(sys_file::Column::ReservationExpiresAt)
            .limit(limit)
            .all(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// Move an expired upload into a cleanup tombstone without deleting the
    /// object yet. The new grace window protects against a late PUT completing
    /// after the original uploader has stopped renewing its lease.
    pub async fn begin_expired_cleanup(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        now: chrono::DateTime<chrono::Utc>,
        cleanup_after: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        let result = sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::UploadStatus,
                sea_orm::sea_query::Expr::value(sys_file::Model::UPLOAD_STATUS_CLEANUP),
            )
            .col_expr(
                sys_file::Column::ReservationExpiresAt,
                sea_orm::sea_query::Expr::value(cleanup_after),
            )
            .col_expr(
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
            .filter(sys_file::Column::ReservationExpiresAt.lte(now))
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(result.rows_affected == 1)
    }

    pub async fn delete_expired_cleanup(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::delete_many()
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationExpiresAt.lte(now))
            .exec(db)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// Move a failed cleanup attempt behind other due tombstones so a small
    /// set of unavailable objects cannot monopolize every bounded janitor scan.
    pub async fn defer_cleanup_retry(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        due_before: chrono::DateTime<chrono::Utc>,
        retry_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::ReservationExpiresAt,
                sea_orm::sea_query::Expr::value(retry_at),
            )
            .col_expr(
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(due_before),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_UPLOAD_RESERVED))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationExpiresAt.lte(due_before))
            .exec(db)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
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
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }
}
