use ryframe_kernel::AppResult;

use crate::ports::tenant_config::{TenantConfigTransferRecord, TenantConfigTransferTransaction};

use super::TransferOperationRequest;

pub(super) async fn clear_superseded_dead_operation_jobs(
    transaction: &dyn TenantConfigTransferTransaction,
    transfer: &mut TenantConfigTransferRecord,
    operation: &TransferOperationRequest,
) -> AppResult<()> {
    let candidates = match operation {
        TransferOperationRequest::Preview => [
            transfer.apply_background_job_id,
            transfer.rollback_background_job_id,
        ],
        TransferOperationRequest::Apply(_) => [
            transfer.preview_background_job_id,
            transfer.rollback_background_job_id,
        ],
        TransferOperationRequest::Rollback => [
            transfer.preview_background_job_id,
            transfer.apply_background_job_id,
        ],
    }
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(());
    }
    let dead_ids = transaction
        .dead_background_job_ids(&transfer.tenant_id, &candidates)
        .await?;

    // 新操作只废止其他类型的死信执行资格；成功任务指针、应用版本、快照和回滚窗口
    // 均予以保留，因此不会破坏合法回滚链或历史任务关联。
    match operation {
        TransferOperationRequest::Preview => {
            if transfer
                .apply_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.apply_background_job_id = None;
            }
            if transfer
                .rollback_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.rollback_background_job_id = None;
            }
        }
        TransferOperationRequest::Apply(_) => {
            if transfer
                .preview_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.preview_background_job_id = None;
            }
            if transfer
                .rollback_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.rollback_background_job_id = None;
            }
        }
        TransferOperationRequest::Rollback => {
            if transfer
                .preview_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.preview_background_job_id = None;
            }
            if transfer
                .apply_background_job_id
                .is_some_and(|id| dead_ids.contains(&id))
            {
                transfer.apply_background_job_id = None;
            }
        }
    }
    Ok(())
}
