use ryframe_kernel::{ActorContext, AppError, AppResult, ExportQuerySnapshot};

use super::*;

/// 创建任务时固化的选择边界，Worker 不能越过该主键上界。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExportRequestSnapshot {
    pub(super) matched_rows: u64,
    pub(super) upper_id: i64,
}

impl ExportService {
    /// 在任务事务的主库一致性快照中计算筛选结果数量与主键上界。
    pub(super) async fn summarize_request_selection(
        &self,
        transaction: &dyn crate::ExportRequestTransaction,
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

fn validate_export_summary(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_selection_fails_without_creating_a_job() {
        let error = validate_export_summary(
            ExportQuerySnapshot {
                matched_rows: 0,
                upper_id: None,
            },
            500_000,
        )
        .expect_err("空结果必须同步失败");
        assert_eq!(error.error_code().as_str(), "EXPORT_NO_MATCHING_ROWS");
    }

    #[test]
    fn oversized_selection_preserves_count_and_limit() {
        let error = validate_export_summary(
            ExportQuerySnapshot {
                matched_rows: 500_001,
                upper_id: Some(900_001),
            },
            500_000,
        )
        .expect_err("超过上限必须同步失败");
        assert!(matches!(
            error,
            AppError::ExportRowLimitExceeded {
                matched_rows: 500_001,
                limit: 500_000,
            }
        ));
    }

    #[test]
    fn non_empty_selection_requires_a_positive_upper_id() {
        let error = validate_export_summary(
            ExportQuerySnapshot {
                matched_rows: 1,
                upper_id: None,
            },
            500_000,
        )
        .expect_err("非空结果必须具备主键上界");
        assert!(matches!(error, AppError::Database(_)));

        let snapshot = validate_export_summary(
            ExportQuerySnapshot {
                matched_rows: 8,
                upper_id: Some(99),
            },
            500_000,
        )
        .expect("合法选择应生成快照");
        assert_eq!(snapshot.matched_rows, 8);
        assert_eq!(snapshot.upper_id, 99);
    }
}
