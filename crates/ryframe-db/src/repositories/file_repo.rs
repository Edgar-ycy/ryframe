use async_trait::async_trait;
use ryframe_core::repository::{PageResult, Repository, ValidatedPageQuery};
use ryframe_kernel::{AppError, AppResult};
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
        query: ValidatedPageQuery,
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
    /// 读取主数据库的 UTC 时钟，确保每个应用节点对租约和过期作出的决策均使用
    /// 同一权威来源。
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

    /// 提交不代表 HTTP 请求成功的上传预留协调事务。
    ///
    /// 上传预留必须先持久化，随后才能在数据库事务外写入对象存储；这里故意不绑定
    /// 成功审计，最终 `ready` 状态会与 `audit.operation` Outbox 原子提交。
    pub async fn commit_upload_reservation(&self, txn: DatabaseTransaction) -> AppResult<()> {
        txn.commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在调用方事务内软删除文件元数据。
    pub async fn delete_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<()> {
        let result = sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::DelFlag,
                sea_orm::sea_query::Expr::value(sys_file::Model::DEL_FLAG_DELETED),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .exec(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected == 0 {
            return Err(AppError::NotFound("文件不存在".into()));
        }
        Ok(())
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

    /// 按权威 SHA-256 摘要查找已完成上传的文件。
    pub async fn find_by_sha256(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        bucket: &str,
        file_sha256: &str,
    ) -> AppResult<Option<sys_file::Model>> {
        Self::find_by_sha256_query(tenant_id, bucket, file_sha256)
            .one(db)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    pub async fn find_by_sha256_any_status_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        bucket: &str,
        file_sha256: &str,
    ) -> AppResult<Option<sys_file::Model>> {
        Self::find_by_sha256_any_status_query(tenant_id, bucket, file_sha256)
            .lock(LockType::Update)
            .one(txn)
            .await
            .map_err(|e| AppError::Database(e.to_string()))
    }

    fn find_by_sha256_query(
        tenant_id: &str,
        bucket: &str,
        file_sha256: &str,
    ) -> Select<sys_file::Entity> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::FileSha256.eq(file_sha256))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
    }

    fn find_by_sha256_any_status_query(
        tenant_id: &str,
        bucket: &str,
        file_sha256: &str,
    ) -> Select<sys_file::Entity> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::FileSha256.eq(file_sha256))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
    }

    pub async fn find_by_id_any_status(
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
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在事务中锁定尚未软删除的文件元数据；可见性由上传状态决定。
    pub async fn find_by_id_any_status_for_update(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<sys_file::Model>> {
        sys_file::Entity::find_by_id(id)
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(txn)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 将无引用的已完成头像文件改为可恢复的延迟清理墓碑。
    ///
    /// 墓碑在宽限期内可被并发的头像更新恢复；宽限期届满后才由全局清理器删除
    /// 对象和元数据，避免把同一内容去重上传的临界请求误删。
    pub async fn mark_avatar_orphan_for_cleanup_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        now: chrono::DateTime<chrono::Utc>,
        cleanup_after: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::UploadStatus,
                sea_orm::sea_query::Expr::value(sys_file::Model::UPLOAD_STATUS_CLEANUP),
            )
            .col_expr(
                sys_file::Column::ReservationToken,
                sea_orm::sea_query::Expr::value(Option::<String>::None),
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
            .filter(sys_file::Column::Bucket.eq("avatar"))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 将未被导入任务引用的私有导入文件改为可恢复的延迟清理墓碑。
    pub async fn mark_import_orphan_for_cleanup_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        now: chrono::DateTime<chrono::Utc>,
        cleanup_after: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::UploadStatus,
                sea_orm::sea_query::Expr::value(sys_file::Model::UPLOAD_STATUS_CLEANUP),
            )
            .col_expr(
                sys_file::Column::ReservationToken,
                sea_orm::sea_query::Expr::value(Option::<String>::None),
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
            .filter(sys_file::Column::Bucket.eq("imports"))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
            .filter(
                Condition::all()
                    .add(
                        sea_orm::sea_query::Expr::cust_with_values(
                            "NOT EXISTS (SELECT 1 FROM sys_user_import_job import_job WHERE import_job.tenant_id = sys_file.tenant_id AND (import_job.source_file_id = sys_file.id OR import_job.error_report_file_id = sys_file.id))",
                            std::iter::empty::<sea_orm::Value>(),
                        ),
                    ),
            )
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在延迟清理到期前恢复即将被新导入任务引用的私有文件。
    pub async fn restore_import_file_for_reference_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
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
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::Bucket.eq("imports"))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationToken.is_null())
            .filter(sea_orm::sea_query::Expr::cust(
                "reservation_expires_at > UTC_TIMESTAMP(6)",
            ))
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在上传去重事务内恢复仍处于宽限期、尚未被清理器声明的文件。
    pub async fn restore_file_for_reference_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        bucket: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
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
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(updated_at),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::Bucket.eq(bucket))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationToken.is_null())
            .filter(sea_orm::sea_query::Expr::cust(
                "reservation_expires_at > UTC_TIMESTAMP(6)",
            ))
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在延迟清理尚未到期时恢复头像文件，使并发的去重上传能够安全复用它。
    pub async fn restore_avatar_file_for_reference_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
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
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(now),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::Bucket.eq("avatar"))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationToken.is_null())
            .filter(sea_orm::sea_query::Expr::cust(
                "reservation_expires_at > UTC_TIMESTAMP(6)",
            ))
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 在调用方事务内声明一个仍然到期的导入文件进入最终清理。
    ///
    /// 除租户、文件、bucket、软删除状态和上传状态外，该更新还会以同一截止时间
    /// 重新验证所有导入任务引用，避免初始候选查询后新增引用造成误删。非空令牌一经
    /// 提交，业务引用便不能再恢复该记录；声明过期后由全局清理器接管。
    pub async fn claim_ready_expired_import_artifact_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
        claim_token: &str,
        expired_before: chrono::DateTime<chrono::Utc>,
        claim_until: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::UploadStatus,
                sea_orm::sea_query::Expr::value(sys_file::Model::UPLOAD_STATUS_CLEANUP),
            )
            .col_expr(
                sys_file::Column::ReservationToken,
                sea_orm::sea_query::Expr::value(Some(claim_token.to_owned())),
            )
            .col_expr(
                sys_file::Column::ReservationExpiresAt,
                sea_orm::sea_query::Expr::value(claim_until),
            )
            .col_expr(
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::cust("UTC_TIMESTAMP(6)"),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::Bucket.eq("imports"))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
            .filter(sys_file::Column::ReservationToken.is_null())
            .filter(sea_orm::sea_query::Expr::cust_with_values(
                "(EXISTS (SELECT 1 FROM sys_user_import_job expired \
                   WHERE expired.tenant_id = sys_file.tenant_id \
                     AND expired.status IN ('succeeded', 'partial', 'failed', 'cancelled') \
                     AND expired.completed_at < ? \
                     AND (expired.source_file_id = sys_file.id OR expired.error_report_file_id = sys_file.id)) \
                 OR (sys_file.created_at < ? AND NOT EXISTS (SELECT 1 FROM sys_user_import_job referenced \
                   WHERE referenced.tenant_id = sys_file.tenant_id \
                     AND (referenced.source_file_id = sys_file.id OR referenced.error_report_file_id = sys_file.id)))) \
                 AND NOT EXISTS (SELECT 1 FROM sys_user_import_job retained \
                   WHERE retained.tenant_id = sys_file.tenant_id \
                     AND (retained.source_file_id = sys_file.id OR retained.error_report_file_id = sys_file.id) \
                     AND (retained.status NOT IN ('succeeded', 'partial', 'failed', 'cancelled') \
                          OR retained.completed_at IS NULL OR retained.completed_at >= ?))",
                [
                    sea_orm::Value::from(expired_before.naive_utc()),
                    sea_orm::Value::from(expired_before.naive_utc()),
                    sea_orm::Value::from(expired_before.naive_utc()),
                ],
            ))
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    pub async fn mark_ready<C>(
        &self,
        db: &C,
        tenant_id: &str,
        id: i64,
        reservation_token: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
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
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(updated_at),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
            .filter(sys_file::Column::ReservationToken.eq(reservation_token))
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(result.rows_affected == 1)
    }

    /// 使用所有权令牌的比较并设置操作延长活动上传租约。
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
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
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
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
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
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
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

    /// 将过期上传移入清理墓碑，暂不删除对象。新的宽限期可防止原上传者停止续期后
    /// 延迟的 PUT 仍完成。
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
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_PENDING))
            .filter(sys_file::Column::ReservationExpiresAt.lte(now))
            .exec(db)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(result.rows_affected == 1)
    }

    /// 原子声明一条已经过宽限期的清理墓碑。
    ///
    /// 非空令牌表示对象已经进入最终清理，任何业务引用都不得再恢复该记录。过期令牌
    /// 可以被其他清理实例接管，使进程崩溃不会永久卡住墓碑。
    pub async fn claim_expired_cleanup(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        claim_token: &str,
        claimed_at: chrono::DateTime<chrono::Utc>,
        claim_until: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::ReservationToken,
                sea_orm::sea_query::Expr::value(Some(claim_token.to_owned())),
            )
            .col_expr(
                sys_file::Column::ReservationExpiresAt,
                sea_orm::sea_query::Expr::value(claim_until),
            )
            .col_expr(
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(claimed_at),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationExpiresAt.lte(claimed_at))
            .exec(db)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 对象删除成功后，仅由仍持有清理令牌的实例删除元数据。
    pub async fn complete_cleanup_claim(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        claim_token: &str,
    ) -> AppResult<bool> {
        sys_file::Entity::delete_many()
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationToken.eq(claim_token))
            .exec(db)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(|error| AppError::Database(error.to_string()))
    }

    /// 将失败的清理声明延后到其他到期墓碑之后，避免少量不可用对象独占每次有界扫描。
    pub async fn defer_cleanup_claim(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
        claim_token: &str,
        updated_at: chrono::DateTime<chrono::Utc>,
        retry_at: chrono::DateTime<chrono::Utc>,
    ) -> AppResult<bool> {
        sys_file::Entity::update_many()
            .col_expr(
                sys_file::Column::ReservationExpiresAt,
                sea_orm::sea_query::Expr::value(retry_at),
            )
            .col_expr(
                sys_file::Column::UpdatedAt,
                sea_orm::sea_query::Expr::value(updated_at),
            )
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_CLEANUP))
            .filter(sys_file::Column::ReservationToken.eq(claim_token))
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
