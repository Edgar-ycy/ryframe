use ryframe_kernel::{ActorContext, AppError, AppResult, ExportQuerySnapshot};

use crate::ports::export::ExportRequestTransaction;

use super::*;

/// 创建任务时固化的选择边界，Worker 不能越过该主键上界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportRequestSnapshot {
    pub matched_rows: u64,
    pub upper_id: i64,
}

impl ExportService {
    /// 在任务事务的主库一致性快照中计算筛选结果数量与主键上界。
    pub(super) async fn summarize_request_selection(
        &self,
        transaction: &dyn ExportRequestTransaction,
        actor: &ActorContext,
        selection: &ExportSelection,
    ) -> AppResult<ExportRequestSnapshot> {
        let tenant_id = crate::validated_tenant_id(actor)?;
        let summary = transaction
            .summarize_selection(tenant_id, actor, selection)
            .await?;
        validate_export_summary(summary, self.export_max_rows)
    }
}

pub fn validate_export_summary(
    summary: ExportQuerySnapshot,
    maximum_rows: usize,
) -> AppResult<ExportRequestSnapshot> {
    if summary.matched_rows == 0 {
        return Err(AppError::ExportNoMatchingRows(
            "当前筛选与数据权限下没有可导出的记录".into(),
        ));
    }
    let limit = u64::try_from(maximum_rows)
        .map_err(|_| AppError::Config("导出行数上限无法转换为无符号整数".into()))?;
    if summary.matched_rows > limit {
        return Err(AppError::ExportRowLimitExceeded {
            matched_rows: summary.matched_rows,
            limit,
        });
    }
    let upper_id = summary
        .upper_id
        .filter(|id| *id > 0)
        .ok_or_else(|| AppError::Database("非空导出选择缺少有效主键上界".into()))?;
    Ok(ExportRequestSnapshot {
        matched_rows: summary.matched_rows,
        upper_id,
    })
}
