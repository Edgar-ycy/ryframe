use std::{future::Future, path::Path, pin::Pin};

use ryframe_kernel::AppResult;

pub const SPREADSHEET_MAX_DATA_ROWS: u64 = 1_048_575;
pub type SpreadsheetRow = serde_json::Value;
pub type SpreadsheetDocumentFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

pub struct SpreadsheetImportRow {
    pub row_number: usize,
    pub value: Result<SpreadsheetRow, String>,
}

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
    fn sha256(&self) -> &str;
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

/// XLSX 校验、解析及小型模板和报告生成端口。
pub trait SpreadsheetDocumentProcessor: Send + Sync {
    fn validate_source(
        &self,
        data: Vec<u8>,
        expected_headers: &'static [(&'static str, &'static str)],
    ) -> SpreadsheetDocumentFuture<'_, Vec<u8>>;

    fn read_rows(
        &self,
        data: Vec<u8>,
        expected_headers: &'static [(&'static str, &'static str)],
    ) -> SpreadsheetDocumentFuture<'_, Vec<SpreadsheetImportRow>>;

    fn export_template(
        &self,
        sheet_name: &'static str,
        headers: &'static [(&'static str, &'static str)],
        reference_sheet_name: &'static str,
        reference_header: &'static str,
        reference_values: Vec<String>,
    ) -> SpreadsheetDocumentFuture<'_, Vec<u8>>;

    fn export_rows(
        &self,
        rows: Vec<SpreadsheetRow>,
        sheet_name: &'static str,
        headers: &'static [(&'static str, &'static str)],
    ) -> SpreadsheetDocumentFuture<'_, Vec<u8>>;
}
