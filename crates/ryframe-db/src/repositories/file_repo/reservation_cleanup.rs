use crate::{DbResultExt, entities::sys_file};
use ryframe_kernel::AppResult;
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};

use super::FileRepository;

impl FileRepository {
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
}
