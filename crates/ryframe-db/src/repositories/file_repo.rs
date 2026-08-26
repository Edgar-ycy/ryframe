use crate::DbResultExt;
use async_trait::async_trait;
use ryframe_kernel::{AppError, AppResult, PageResult, ValidatedPageQuery};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseConnection,
    DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Select,
    sea_query::LockType,
};

use crate::{Repository, entities::sys_file};

mod cleanup;

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
            .db()
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
    /// 读取导出清理需要的文件元数据，包括已经软删除的历史记录。
    pub async fn find_file_for_purge(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<sys_file::Model>> {
        sys_file::Entity::find_by_id(id)
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .one(db)
            .await
            .db()
    }

    /// 硬删除仅由指定导出任务引用的结果文件元数据。
    ///
    /// 新基线通过 `uq_export_job_result_file` 消除跨导出任务共享；应用启动时的 schema
    /// 指纹校验保证清理器不会在缺少该约束的数据库上运行。
    pub async fn hard_delete_exclusive_export_file_in_txn(
        &self,
        txn: &DatabaseTransaction,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<bool> {
        sys_file::Entity::delete_many()
            .filter(sys_file::Column::Id.eq(id))
            .filter(sys_file::Column::TenantId.eq(tenant_id))
            .filter(sys_file::Column::Bucket.eq("exports"))
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .db()
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
        txn.commit().await.db()
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
            .db()?;
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
            .db()
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
            .db()
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
            .db()
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
            .db()
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
            .db()
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
            .db()?;
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
            .db()
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
            .db()?;
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
            .db()
    }

    /// 查找已完成上传、经过最长任务运行窗口后仍未被配置包或回滚快照引用的内部文件。
    ///
    /// 这里只产生候选项；调用方仍须在租户行锁和文件行锁下再次验证引用，才能进入延迟清理。
    pub async fn find_stale_unreferenced_config_packages(
        &self,
        db: &DatabaseConnection,
        ready_before: chrono::DateTime<chrono::Utc>,
        limit: u64,
    ) -> AppResult<Vec<sys_file::Model>> {
        sys_file::Entity::find()
            .filter(sys_file::Column::Bucket.eq("config-packages"))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
            .filter(sys_file::Column::ReservationToken.is_null())
            .filter(sys_file::Column::UpdatedAt.lte(ready_before))
            .filter(
                Condition::all()
                    .add(sea_orm::sea_query::Expr::cust(
                        "NOT EXISTS (SELECT 1 FROM sys_tenant_config_bundle bundle WHERE bundle.tenant_id = sys_file.tenant_id AND bundle.file_id = sys_file.id)",
                    ))
                    .add(sea_orm::sea_query::Expr::cust(
                        "NOT EXISTS (SELECT 1 FROM sys_tenant_config_transfer transfer WHERE transfer.tenant_id = sys_file.tenant_id AND transfer.snapshot_file_id = sys_file.id)",
                    )),
            )
            .order_by_asc(sys_file::Column::UpdatedAt)
            .order_by_asc(sys_file::Column::Id)
            .limit(limit)
            .all(db)
            .await
            .db()
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
            .db()?;
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
            .db()
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
            .db()
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
            .db()
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
            .db()
    }
}
