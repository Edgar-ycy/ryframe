#![forbid(unsafe_code)]

//! RyFrame 的 Excel 导入导出组件。

pub use ryframe_kernel::{AppError, AppResult};

mod excel;

pub use excel::{
    ExcelArtifact, ExcelBatchProgress, ExcelExporter, ExcelImportRow, ExcelImporter,
    IncrementalExcelWriter, XLSX_MAX_DATA_ROWS,
};
