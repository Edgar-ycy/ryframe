use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use ryframe_utils::snowflake;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    sea_query::{Expr, LockType},
};
use serde_json::Value;

use crate::entities::{export_job, tenant};

mod deletion;

pub use deletion::MarkExportJobsDeletePending;

/// 创建导出任务时写入的不可变请求快照。
#[derive(Clone, Debug)]
pub struct CreateExportJob {
    pub tenant_id: String,
    pub requester_id: i64,
    pub resource: String,
    pub background_job_id: i64,
    pub request_params: Value,
    pub request_version: i32,
    pub permission_code: String,
    pub authorization_fingerprint: String,
    pub request_fingerprint: String,
    pub snapshot_at: DateTime<Utc>,
    pub upper_id: i64,
    pub matched_rows: i64,
}

/// 租户并发门禁对一次启动请求的判定。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportStartDisposition {
    Started,
    AlreadyRunning,
    ConcurrencyLimited,
    NotRunnable,
}

/// 将导出任务标记为成功时写入的结果元数据。
#[derive(Clone, Debug)]
pub struct MarkExportJobSucceeded {
    pub id: i64,
    pub file_id: i64,
    pub file_name: String,
    pub content_type: String,
    pub file_size: i64,
    pub expires_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

/// 异步导出任务仓储。
pub struct ExportJobRepository;

impl ExportJobRepository {
    /// 按公开任务 ID 强一致读取导出状态。
    pub async fn find_by_id(
        &self,
        db: &DatabaseConnection,
        id: i64,
    ) -> AppResult<Option<export_job::Model>> {
        export_job::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 在任务入队的同一事务内记录导出请求。
    pub async fn create_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        command: CreateExportJob,
        now: DateTime<Utc>,
    ) -> AppResult<export_job::Model> {
        validate_create_command(&command)?;
        export_job::ActiveModel {
            id: Set(snowflake::try_next_snowflake_id()?),
            tenant_id: Set(command.tenant_id),
            requester_id: Set(command.requester_id),
            resource: Set(command.resource),
            background_job_id: Set(command.background_job_id),
            request_params: Set(command.request_params),
            request_version: Set(command.request_version),
            permission_code: Set(command.permission_code),
            authorization_fingerprint: Set(command.authorization_fingerprint),
            request_fingerprint: Set(command.request_fingerprint.clone()),
            active_request_fingerprint: Set(Some(command.request_fingerprint)),
            snapshot_at: Set(command.snapshot_at),
            upper_id: Set(command.upper_id),
            matched_rows: Set(command.matched_rows),
            exported_rows: Set(0),
            status: Set(export_job::Model::STATUS_QUEUED.to_owned()),
            result_file_id: Set(None),
            result_file_name: Set(None),
            content_type: Set(None),
            file_size: Set(None),
            expires_at: Set(None),
            error_message: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
            completed_at: Set(None),
            notification_read_at: Set(None),
            delete_pending_at: Set(None),
        }
        .insert(transaction)
        .await
        .map_err(database_error)
    }

    /// 在租户行锁保护下复用同一规范化请求的排队或运行任务。
    pub async fn find_active_by_fingerprint_for_update(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        requester_id: i64,
        request_fingerprint: &str,
    ) -> AppResult<Option<export_job::Model>> {
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        export_job::Entity::find()
            .filter(export_job::Column::TenantId.eq(tenant_id))
            .filter(export_job::Column::RequesterId.eq(requester_id))
            .filter(export_job::Column::ActiveRequestFingerprint.eq(request_fingerprint))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .filter(export_job::Column::Status.is_in([
                export_job::Model::STATUS_QUEUED,
                export_job::Model::STATUS_RUNNING,
            ]))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    /// 读取当前申请人在当前租户可见的导出任务。
    pub async fn find_by_id_for_requester(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        requester_id: i64,
        id: i64,
    ) -> AppResult<Option<export_job::Model>> {
        visible_for_requester_query(tenant_id, requester_id)
            .filter(export_job::Column::Id.eq(id))
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 按创建时间倒序读取申请人最近的导出任务。
    pub async fn list_for_requester(
        &self,
        db: &DatabaseConnection,
        tenant_id: &str,
        requester_id: i64,
        limit: u64,
    ) -> AppResult<Vec<export_job::Model>> {
        visible_for_requester_query(tenant_id, requester_id)
            .order_by_desc(export_job::Column::CreatedAt)
            .order_by_desc(export_job::Column::Id)
            .limit(limit.clamp(1, 100))
            .all(db)
            .await
            .map_err(database_error)
    }

    /// 将申请人已经实际看到的成功或失败通知幂等标记为已查看。
    pub async fn mark_notifications_read<C>(
        &self,
        db: &C,
        tenant_id: &str,
        requester_id: i64,
        ids: &[i64],
        now: DateTime<Utc>,
    ) -> AppResult<u64>
    where
        C: ConnectionTrait,
    {
        export_job::Entity::update_many()
            .col_expr(export_job::Column::NotificationReadAt, Expr::value(now))
            .filter(export_job::Column::TenantId.eq(tenant_id))
            .filter(export_job::Column::RequesterId.eq(requester_id))
            .filter(export_job::Column::Id.is_in(ids.iter().copied()))
            .filter(export_job::Column::NotificationReadAt.is_null())
            .filter(export_job::Column::DeletePendingAt.is_null())
            .filter(export_job::Column::Status.is_in([
                export_job::Model::STATUS_SUCCEEDED,
                export_job::Model::STATUS_FAILED,
            ]))
            .exec(db)
            .await
            .map(|result| result.rows_affected)
            .map_err(database_error)
    }

    /// 根据内部后台任务定位对应的导出任务，供 Worker 使用。
    pub async fn find_by_background_job_id(
        &self,
        db: &DatabaseConnection,
        background_job_id: i64,
    ) -> AppResult<Option<export_job::Model>> {
        export_job::Entity::find()
            .filter(export_job::Column::BackgroundJobId.eq(background_job_id))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .one(db)
            .await
            .map_err(database_error)
    }

    /// 在结果落库事务内锁定导出任务，串行化对象写入和最终状态提交。
    pub async fn find_by_id_for_update_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        id: i64,
    ) -> AppResult<Option<export_job::Model>> {
        export_job::Entity::find_by_id(id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)
    }

    /// 在租户行锁下启动导出，保证同一租户同时最多运行指定数量的任务。
    pub async fn try_mark_running_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        id: i64,
        tenant_id: &str,
        maximum_running: u64,
        now: DateTime<Utc>,
    ) -> AppResult<ExportStartDisposition> {
        tenant::Entity::find()
            .filter(tenant::Column::TenantId.eq(tenant_id))
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let current = export_job::Entity::find_by_id(id)
            .lock(LockType::Update)
            .one(transaction)
            .await
            .map_err(database_error)?
            .ok_or_else(|| AppError::NotFound("导出任务不存在".into()))?;
        if current.tenant_id != tenant_id {
            return Ok(ExportStartDisposition::NotRunnable);
        }
        let running = export_job::Entity::find()
            .filter(export_job::Column::TenantId.eq(tenant_id))
            .filter(export_job::Column::Status.eq(export_job::Model::STATUS_RUNNING))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .count(transaction)
            .await
            .map_err(database_error)?;
        let disposition = decide_export_start(
            &current.status,
            current.delete_pending_at.is_some(),
            running,
            maximum_running,
        );
        if disposition != ExportStartDisposition::Started {
            return Ok(disposition);
        }
        let result = export_job::Entity::update_many()
            .col_expr(
                export_job::Column::Status,
                Expr::value(export_job::Model::STATUS_RUNNING),
            )
            .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
            .filter(export_job::Column::Id.eq(id))
            .filter(export_job::Column::Status.eq(export_job::Model::STATUS_QUEUED))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .exec(transaction)
            .await
            .map_err(database_error)?;
        Ok(if result.rows_affected == 1 {
            ExportStartDisposition::Started
        } else {
            ExportStartDisposition::NotRunnable
        })
    }

    /// 只允许执行中的任务按单调递增方式记录已写入行数。
    pub async fn update_exported_rows<C>(
        &self,
        db: &C,
        id: i64,
        exported_rows: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        export_job::Entity::update_many()
            .col_expr(export_job::Column::ExportedRows, Expr::value(exported_rows))
            .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
            .filter(export_job::Column::Id.eq(id))
            .filter(export_job::Column::Status.eq(export_job::Model::STATUS_RUNNING))
            .filter(export_job::Column::ExportedRows.lt(exported_rows))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .exec(db)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(database_error)
    }

    /// 将 Worker 成功生成的文件元数据固化为可下载结果。
    pub async fn mark_succeeded_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        command: MarkExportJobSucceeded,
    ) -> AppResult<bool> {
        let result = export_job::Entity::update_many()
            .col_expr(
                export_job::Column::Status,
                Expr::value(export_job::Model::STATUS_SUCCEEDED),
            )
            .col_expr(
                export_job::Column::ResultFileId,
                Expr::value(command.file_id),
            )
            .col_expr(
                export_job::Column::ResultFileName,
                Expr::value(Some(command.file_name)),
            )
            .col_expr(
                export_job::Column::ContentType,
                Expr::value(Some(command.content_type)),
            )
            .col_expr(export_job::Column::FileSize, Expr::value(command.file_size))
            .col_expr(
                export_job::Column::ExpiresAt,
                Expr::value(command.expires_at),
            )
            .col_expr(
                export_job::Column::UpdatedAt,
                Expr::value(command.completed_at),
            )
            .col_expr(
                export_job::Column::CompletedAt,
                Expr::value(command.completed_at),
            )
            .col_expr(
                export_job::Column::ActiveRequestFingerprint,
                Expr::value(Option::<String>::None),
            )
            .filter(export_job::Column::Id.eq(command.id))
            .filter(export_job::Column::Status.eq(export_job::Model::STATUS_RUNNING))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .exec(transaction)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 在仍可执行时记录明确失败原因，取消后的任务保持取消状态。
    pub async fn mark_failed<C>(
        &self,
        db: &C,
        id: i64,
        error_message: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let result = export_job::Entity::update_many()
            .col_expr(
                export_job::Column::Status,
                Expr::value(export_job::Model::STATUS_FAILED),
            )
            .col_expr(
                export_job::Column::ErrorMessage,
                Expr::value(Some(truncate_error(error_message))),
            )
            .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
            .col_expr(export_job::Column::CompletedAt, Expr::value(now))
            .col_expr(
                export_job::Column::ActiveRequestFingerprint,
                Expr::value(Option::<String>::None),
            )
            .filter(export_job::Column::Id.eq(id))
            .filter(export_job::Column::Status.eq(export_job::Model::STATUS_RUNNING))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 将一次可重试失败重新置为排队状态，保留错误信息供任务详情查询。
    pub async fn mark_queued_after_failure<C>(
        &self,
        db: &C,
        id: i64,
        error_message: &str,
        now: DateTime<Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let result = export_job::Entity::update_many()
            .col_expr(
                export_job::Column::Status,
                Expr::value(export_job::Model::STATUS_QUEUED),
            )
            .col_expr(
                export_job::Column::ErrorMessage,
                Expr::value(Some(truncate_error(error_message))),
            )
            .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
            .filter(export_job::Column::Id.eq(id))
            .filter(export_job::Column::Status.eq(export_job::Model::STATUS_RUNNING))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 取消尚未完成的任务。Worker 会在状态切换前复核，避免取消后继续写入结果。
    pub async fn cancel_for_requester<C>(
        &self,
        db: &C,
        tenant_id: &str,
        requester_id: i64,
        id: i64,
        now: DateTime<Utc>,
    ) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let result = export_job::Entity::update_many()
            .col_expr(
                export_job::Column::Status,
                Expr::value(export_job::Model::STATUS_CANCELLED),
            )
            .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
            .col_expr(export_job::Column::CompletedAt, Expr::value(now))
            .col_expr(
                export_job::Column::ActiveRequestFingerprint,
                Expr::value(Option::<String>::None),
            )
            .filter(export_job::Column::Id.eq(id))
            .filter(export_job::Column::TenantId.eq(tenant_id))
            .filter(export_job::Column::RequesterId.eq(requester_id))
            .filter(export_job::Column::Status.is_in([
                export_job::Model::STATUS_QUEUED,
                export_job::Model::STATUS_RUNNING,
            ]))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }

    /// 按稳定主键游标读取一批已到期、但尚未清理结果文件的任务。
    pub async fn list_expired_succeeded_after_id(
        &self,
        db: &DatabaseConnection,
        now: DateTime<Utc>,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<export_job::Model>> {
        let mut query = export_job::Entity::find()
            .filter(export_job::Column::Status.eq(export_job::Model::STATUS_SUCCEEDED))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .filter(export_job::Column::ExpiresAt.lte(now))
            .order_by_asc(export_job::Column::Id)
            .limit(limit.clamp(1, 1_000));
        if let Some(after_id) = after_id {
            query = query.filter(export_job::Column::Id.gt(after_id));
        }
        query.all(db).await.map_err(database_error)
    }

    /// 在文件对象与元数据均清理完成后标记导出任务过期。
    pub async fn mark_expired<C>(&self, db: &C, id: i64, now: DateTime<Utc>) -> AppResult<bool>
    where
        C: ConnectionTrait,
    {
        let result = export_job::Entity::update_many()
            .col_expr(
                export_job::Column::Status,
                Expr::value(export_job::Model::STATUS_EXPIRED),
            )
            .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
            .col_expr(
                export_job::Column::ResultFileId,
                Expr::value(Option::<i64>::None),
            )
            .col_expr(
                export_job::Column::ResultFileName,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                export_job::Column::ContentType,
                Expr::value(Option::<String>::None),
            )
            .col_expr(
                export_job::Column::FileSize,
                Expr::value(Option::<i64>::None),
            )
            .col_expr(
                export_job::Column::ActiveRequestFingerprint,
                Expr::value(Option::<String>::None),
            )
            .filter(export_job::Column::Id.eq(id))
            .filter(export_job::Column::Status.eq(export_job::Model::STATUS_SUCCEEDED))
            .filter(export_job::Column::ExpiresAt.lte(now))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .exec(db)
            .await
            .map_err(database_error)?;
        Ok(result.rows_affected == 1)
    }
}

fn validate_create_command(command: &CreateExportJob) -> AppResult<()> {
    if command.tenant_id.is_empty() || command.tenant_id.len() > 64 {
        return Err(AppError::Validation("导出任务租户标识无效".into()));
    }
    if command.requester_id <= 0 || command.background_job_id <= 0 {
        return Err(AppError::Validation("导出任务关联标识必须为正数".into()));
    }
    if command.upper_id <= 0 || command.matched_rows <= 0 || command.request_version <= 0 {
        return Err(AppError::Validation(
            "导出任务主键上界和匹配行数必须为正数".into(),
        ));
    }
    for (name, value, maximum) in [
        ("resource", command.resource.as_str(), 64),
        ("permission_code", command.permission_code.as_str(), 128),
    ] {
        if value.is_empty() || value.len() > maximum {
            return Err(AppError::Validation(format!(
                "导出任务 {name} 长度必须介于 1 和 {maximum} 之间"
            )));
        }
    }
    for (name, value) in [
        (
            "authorization_fingerprint",
            command.authorization_fingerprint.as_str(),
        ),
        ("request_fingerprint", command.request_fingerprint.as_str()),
    ] {
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::Validation(format!(
                "导出任务 {name} 必须是 64 位十六进制指纹"
            )));
        }
    }
    Ok(())
}

fn visible_for_requester_query(
    tenant_id: &str,
    requester_id: i64,
) -> sea_orm::Select<export_job::Entity> {
    export_job::Entity::find()
        .filter(export_job::Column::TenantId.eq(tenant_id))
        .filter(export_job::Column::RequesterId.eq(requester_id))
        .filter(export_job::Column::DeletePendingAt.is_null())
}

fn truncate_error(error: &str) -> String {
    const MAX_ERROR_BYTES: usize = 4_000;
    if error.len() <= MAX_ERROR_BYTES {
        return error.to_owned();
    }
    let mut end = MAX_ERROR_BYTES;
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &error[..end])
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

fn decide_export_start(
    status: &str,
    delete_pending: bool,
    running: u64,
    maximum_running: u64,
) -> ExportStartDisposition {
    if delete_pending {
        return ExportStartDisposition::NotRunnable;
    }
    if status == export_job::Model::STATUS_RUNNING {
        return ExportStartDisposition::AlreadyRunning;
    }
    if status != export_job::Model::STATUS_QUEUED {
        return ExportStartDisposition::NotRunnable;
    }
    if running >= maximum_running {
        ExportStartDisposition::ConcurrencyLimited
    } else {
        ExportStartDisposition::Started
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::{DatabaseBackend, QueryTrait};

    use super::*;

    #[test]
    fn requester_queries_hide_delete_tombstones() {
        let statement = visible_for_requester_query("tenant-a", 7).build(DatabaseBackend::MySql);
        assert!(statement.sql.contains("delete_pending_at` IS NULL"));
    }

    #[test]
    fn start_gate_rejects_duplicate_worker_and_third_tenant_export() {
        assert_eq!(
            decide_export_start(export_job::Model::STATUS_RUNNING, false, 1, 2),
            ExportStartDisposition::AlreadyRunning
        );
        assert_eq!(
            decide_export_start(export_job::Model::STATUS_QUEUED, false, 2, 2),
            ExportStartDisposition::ConcurrencyLimited
        );
        assert_eq!(
            decide_export_start(export_job::Model::STATUS_QUEUED, false, 1, 2),
            ExportStartDisposition::Started
        );
        assert_eq!(
            decide_export_start(export_job::Model::STATUS_QUEUED, true, 0, 2),
            ExportStartDisposition::NotRunnable
        );
    }
}
