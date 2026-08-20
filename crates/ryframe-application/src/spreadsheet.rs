use std::path::Path;

use ryframe_kernel::AppResult;

pub const SPREADSHEET_MAX_DATA_ROWS: u64 = 1_048_575;
pub type SpreadsheetRow = serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpreadsheetBatchProgress {
    pub batch_rows: u64,
    pub total_rows: u64,
    pub total_input_bytes: u64,
}

/// 增量表格写入器。
pub trait SpreadsheetWriter: Send {
    fn data_rows(&self) -> u64;
    fn input_bytes(&self) -> u64;

    fn append_rows(
        &mut self,
        rows: &mut dyn Iterator<Item = SpreadsheetRow>,
    ) -> AppResult<SpreadsheetBatchProgress>;

    fn finish(self: Box<Self>) -> AppResult<Box<dyn SpreadsheetArtifact>>;
}

/// 已生成表格制品，持有其临时文件生命周期。
pub trait SpreadsheetArtifact: Send {
    fn path(&self) -> &Path;
    fn size(&self) -> u64;
    fn data_rows(&self) -> u64;
    fn input_bytes(&self) -> u64;
}

/// 表格写入器工厂。
pub trait SpreadsheetWriterFactory: Send + Sync {
    fn create(
        &self,
        sheet_name: &'static str,
        headers: &'static [(&'static str, &'static str)],
    ) -> AppResult<Box<dyn SpreadsheetWriter>>;
}
