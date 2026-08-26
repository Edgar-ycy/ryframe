use crate::DbResultExt;
use ryframe_kernel::AppResult;
use sea_orm::{ColumnTrait, Condition, DatabaseTransaction, EntityTrait, QueryFilter};

use crate::entities::sys_file;

use super::FileRepository;

impl FileRepository {
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
            .db()
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
                Condition::all().add(sea_orm::sea_query::Expr::cust_with_values(
                    "NOT EXISTS (SELECT 1 FROM sys_user_import_job import_job WHERE import_job.tenant_id = sys_file.tenant_id AND (import_job.source_file_id = sys_file.id OR import_job.error_report_file_id = sys_file.id))",
                    std::iter::empty::<sea_orm::Value>(),
                )),
            )
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .db()
    }

    /// 将未被配置包或配置迁移记录引用的私有文件改为延迟清理墓碑。
    pub async fn mark_unreferenced_config_package_for_cleanup_in_txn(
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
            .filter(sys_file::Column::Bucket.eq("config-packages"))
            .filter(sys_file::Column::DelFlag.eq(sys_file::Model::DEL_FLAG_NORMAL))
            .filter(sys_file::Column::UploadStatus.eq(sys_file::Model::UPLOAD_STATUS_READY))
            .filter(
                Condition::all()
                    .add(sea_orm::sea_query::Expr::cust(
                        "NOT EXISTS (SELECT 1 FROM sys_tenant_config_bundle bundle WHERE bundle.tenant_id = sys_file.tenant_id AND bundle.file_id = sys_file.id)",
                    ))
                    .add(sea_orm::sea_query::Expr::cust(
                        "NOT EXISTS (SELECT 1 FROM sys_tenant_config_transfer transfer WHERE transfer.tenant_id = sys_file.tenant_id AND transfer.snapshot_file_id = sys_file.id)",
                    )),
            )
            .exec(txn)
            .await
            .map(|result| result.rows_affected == 1)
            .db()
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
            .db()
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
            .db()
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
            .db()
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
            .db()
    }
}
