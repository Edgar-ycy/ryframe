/// 申请导出时由权威数据源计算的稳定选择边界。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportQuerySnapshot {
    pub matched_rows: u64,
    pub upper_id: Option<i64>,
}

/// 导出批次的主键游标窗口。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportCursorWindow<'a> {
    after_id: Option<i64>,
    upper_id: i64,
    limit: u64,
    selected_ids: &'a [i64],
}

impl<'a> ExportCursorWindow<'a> {
    #[must_use]
    pub const fn new(after_id: Option<i64>, upper_id: i64, limit: u64) -> Self {
        Self {
            after_id,
            upper_id,
            limit,
            selected_ids: &[],
        }
    }

    #[must_use]
    pub const fn after_id(self) -> Option<i64> {
        self.after_id
    }

    #[must_use]
    pub const fn upper_id(self) -> i64 {
        self.upper_id
    }

    #[must_use]
    pub const fn limit(self) -> u64 {
        self.limit
    }

    /// 将已校验的选中主键限制带入每个导出批次，不放宽原有数据权限。
    #[must_use]
    pub const fn with_selected_ids(mut self, ids: &'a [i64]) -> Self {
        self.selected_ids = ids;
        self
    }

    #[must_use]
    pub const fn selected_ids(self) -> &'a [i64] {
        self.selected_ids
    }
}
