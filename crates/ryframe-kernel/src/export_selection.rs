/// 申请导出时由权威数据源计算的稳定选择边界。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportQuerySnapshot {
    pub matched_rows: u64,
    pub upper_id: Option<i64>,
}

/// 导出批次的主键游标窗口。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportCursorWindow {
    after_id: Option<i64>,
    upper_id: i64,
    limit: u64,
}

impl ExportCursorWindow {
    #[must_use]
    pub const fn new(after_id: Option<i64>, upper_id: i64, limit: u64) -> Self {
        Self {
            after_id,
            upper_id,
            limit,
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
}

#[cfg(test)]
mod tests {
    use super::ExportCursorWindow;

    #[test]
    fn cursor_window_preserves_bounds_and_is_copy() {
        let window = ExportCursorWindow::new(Some(41), 99, 1_000);
        let copied = window;

        assert_eq!(window.after_id(), Some(41));
        assert_eq!(copied.upper_id(), 99);
        assert_eq!(copied.limit(), 1_000);
    }
}
