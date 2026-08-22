use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DatabaseTransaction, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
    sea_query::{Expr, LockType},
};

use crate::entities::{background_job, export_job};

use super::{ExportJobRepository, database_error};

/// 整批导出记录进入删除墓碑后的事务结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MarkExportJobsDeletePending {
    pub removed_unread_count: u64,
}

impl ExportJobRepository {
    /// 锁定并完整校验一批任务后，一次性写入用户不可见的删除墓碑。
    pub async fn mark_delete_pending_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        requester_id: i64,
        ids: &[i64],
        now: DateTime<Utc>,
    ) -> AppResult<MarkExportJobsDeletePending> {
        // 先读取关联后台任务标识，再按“后台任务 → 公开导出任务”的全局顺序加锁，
        // 与 Worker 的完成迁移保持一致，避免形成反向锁等待。
        let snapshot = export_job::Entity::find()
            .filter(export_job::Column::Id.is_in(ids.iter().copied()))
            .order_by_asc(export_job::Column::Id)
            .all(transaction)
            .await
            .map_err(database_error)?;
        validate_candidate_ownership(&snapshot, ids.len(), tenant_id, requester_id)?;
        let background_ids = snapshot
            .iter()
            .map(|export| export.background_job_id)
            .collect::<Vec<_>>();
        let background_jobs = background_job::Entity::find()
            .filter(background_job::Column::Id.is_in(background_ids))
            .order_by_asc(background_job::Column::Id)
            .lock(LockType::Update)
            .all(transaction)
            .await
            .map_err(database_error)?;
        let exports = export_job::Entity::find()
            .filter(export_job::Column::Id.is_in(ids.iter().copied()))
            .order_by_asc(export_job::Column::Id)
            .lock(LockType::Update)
            .all(transaction)
            .await
            .map_err(database_error)?;
        validate_candidate_ownership(&exports, ids.len(), tenant_id, requester_id)?;
        validate_deletion_candidates(&exports, &background_jobs, now)?;

        let removed_unread_count = exports
            .iter()
            .filter(|export| {
                export.notification_read_at.is_none()
                    && matches!(
                        export.status.as_str(),
                        export_job::Model::STATUS_SUCCEEDED | export_job::Model::STATUS_FAILED
                    )
            })
            .count() as u64;

        let updated = export_job::Entity::update_many()
            .col_expr(export_job::Column::DeletePendingAt, Expr::value(now))
            .col_expr(export_job::Column::UpdatedAt, Expr::value(now))
            .filter(export_job::Column::Id.is_in(ids.iter().copied()))
            .filter(export_job::Column::DeletePendingAt.is_null())
            .exec(transaction)
            .await
            .map_err(database_error)?;
        if updated.rows_affected != ids.len() as u64 {
            return Err(AppError::Conflict("导出任务删除状态已变化".into()));
        }

        Ok(MarkExportJobsDeletePending {
            removed_unread_count,
        })
    }

    /// 按稳定主键游标读取待清理墓碑，供定时任务可靠重试。
    pub async fn list_delete_pending_after_id(
        &self,
        db: &DatabaseConnection,
        after_id: Option<i64>,
        limit: u64,
    ) -> AppResult<Vec<export_job::Model>> {
        let mut query = export_job::Entity::find()
            .filter(export_job::Column::DeletePendingAt.is_not_null())
            .order_by_asc(export_job::Column::Id)
            .limit(limit.clamp(1, 1_000));
        if let Some(after_id) = after_id {
            query = query.filter(export_job::Column::Id.gt(after_id));
        }
        query.all(db).await.map_err(database_error)
    }

    /// 对象与独占文件元数据已删除后，移除仍处于墓碑状态的公开任务记录。
    pub async fn delete_pending_in_transaction(
        &self,
        transaction: &DatabaseTransaction,
        id: i64,
    ) -> AppResult<bool> {
        export_job::Entity::delete_many()
            .filter(export_job::Column::Id.eq(id))
            .filter(export_job::Column::DeletePendingAt.is_not_null())
            .exec(transaction)
            .await
            .map(|result| result.rows_affected == 1)
            .map_err(database_error)
    }
}

pub fn validate_deletion_candidates(
    exports: &[export_job::Model],
    background_jobs: &[background_job::Model],
    now: DateTime<Utc>,
) -> AppResult<()> {
    if exports.iter().any(|export| !export.is_terminal()) {
        return Err(AppError::Conflict(
            "排队或执行中的导出任务必须先取消".into(),
        ));
    }
    let has_active_lease = background_jobs.iter().any(|job| {
        job.lease_owner.is_some() && job.lease_until.is_some_and(|lease_until| lease_until > now)
    });
    if has_active_lease {
        return Err(AppError::Conflict(
            "导出任务仍被 Worker 持有，请稍后重试".into(),
        ));
    }
    Ok(())
}

pub fn validate_candidate_ownership(
    exports: &[export_job::Model],
    requested_count: usize,
    tenant_id: &str,
    requester_id: i64,
) -> AppResult<()> {
    if exports.len() != requested_count
        || exports.iter().any(|export| {
            export.tenant_id != tenant_id
                || export.requester_id != requester_id
                || export.delete_pending_at.is_some()
        })
    {
        return Err(AppError::NotFound("导出任务不存在或不属于当前用户".into()));
    }
    Ok(())
}
