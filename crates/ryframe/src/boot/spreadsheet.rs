use std::sync::Arc;

use ryframe_adapters::excel::{
    ExcelArtifact, ExcelExporter, ExcelImporter, IncrementalExcelWriter,
};
use ryframe_application::ports::spreadsheet::{
    SpreadsheetArtifact, SpreadsheetBatchProgress, SpreadsheetDocumentFuture,
    SpreadsheetDocumentProcessor, SpreadsheetImportRow, SpreadsheetRow, SpreadsheetWriter,
    SpreadsheetWriterFactory,
};
use ryframe_kernel::AppResult;

struct SpreadsheetFactoryBridge;

struct SpreadsheetWriterBridge {
    writer: IncrementalExcelWriter<'static>,
}

struct SpreadsheetArtifactBridge {
    artifact: ExcelArtifact,
}

impl SpreadsheetWriterFactory for SpreadsheetFactoryBridge {
    fn create(
        &self,
        sheet_name: &'static str,
        headers: &'static [(&'static str, &'static str)],
    ) -> AppResult<Box<dyn SpreadsheetWriter>> {
        IncrementalExcelWriter::new(sheet_name, headers).map(|writer| {
            Box::new(SpreadsheetWriterBridge { writer }) as Box<dyn SpreadsheetWriter>
        })
    }
}

impl SpreadsheetDocumentProcessor for SpreadsheetFactoryBridge {
    fn validate_source(
        &self,
        data: Vec<u8>,
        expected_headers: &'static [(&'static str, &'static str)],
    ) -> SpreadsheetDocumentFuture<'_, Vec<u8>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                ExcelImporter::validate_headers_from_bytes(&data, None, expected_headers)?;
                Ok(data)
            })
            .await
            .map_err(|error| {
                ryframe_kernel::AppError::Internal(format!("XLSX 内容校验任务异常结束: {error}"))
            })?
        })
    }

    fn read_rows(
        &self,
        data: Vec<u8>,
        expected_headers: &'static [(&'static str, &'static str)],
    ) -> SpreadsheetDocumentFuture<'_, Vec<SpreadsheetImportRow>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                ExcelImporter::validate_headers_from_bytes(&data, None, expected_headers)?;
                ExcelImporter::read_rows_from_bytes::<SpreadsheetRow>(&data, None).map(|rows| {
                    rows.into_iter()
                        .map(|row| SpreadsheetImportRow {
                            row_number: row.row_number,
                            value: row.value,
                        })
                        .collect()
                })
            })
            .await
            .map_err(|error| {
                ryframe_kernel::AppError::Internal(format!("用户导入解析任务异常结束: {error}"))
            })?
        })
    }

    fn export_template(
        &self,
        sheet_name: &'static str,
        headers: &'static [(&'static str, &'static str)],
        reference_sheet_name: &'static str,
        reference_header: &'static str,
        reference_values: Vec<String>,
    ) -> SpreadsheetDocumentFuture<'_, Vec<u8>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                ExcelExporter::export_template_with_reference(
                    sheet_name,
                    headers,
                    reference_sheet_name,
                    reference_header,
                    &reference_values,
                )
            })
            .await
            .map_err(|error| {
                ryframe_kernel::AppError::Internal(format!("用户导入模板生成任务异常结束: {error}"))
            })?
        })
    }

    fn export_rows(
        &self,
        rows: Vec<SpreadsheetRow>,
        sheet_name: &'static str,
        headers: &'static [(&'static str, &'static str)],
    ) -> SpreadsheetDocumentFuture<'_, Vec<u8>> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                ExcelExporter::export_to_bytes(&rows, sheet_name, headers)
            })
            .await
            .map_err(|error| {
                ryframe_kernel::AppError::Internal(format!("用户导入报告生成任务异常结束: {error}"))
            })?
        })
    }
}

impl SpreadsheetWriter for SpreadsheetWriterBridge {
    fn data_rows(&self) -> u64 {
        self.writer.data_rows()
    }

    fn input_bytes(&self) -> u64 {
        self.writer.input_bytes()
    }

    fn append_rows(
        &mut self,
        rows: &mut dyn Iterator<Item = SpreadsheetRow>,
    ) -> AppResult<SpreadsheetBatchProgress> {
        self.writer
            .append_rows(rows)
            .map(|progress| SpreadsheetBatchProgress {
                batch_rows: progress.batch_rows,
                total_rows: progress.total_rows,
                total_input_bytes: progress.total_input_bytes,
            })
    }

    fn finish(self: Box<Self>) -> AppResult<Box<dyn SpreadsheetArtifact>> {
        self.writer.finish().map(|artifact| {
            Box::new(SpreadsheetArtifactBridge { artifact }) as Box<dyn SpreadsheetArtifact>
        })
    }
}

impl SpreadsheetArtifact for SpreadsheetArtifactBridge {
    fn path(&self) -> &std::path::Path {
        self.artifact.path()
    }

    fn size(&self) -> u64 {
        self.artifact.size()
    }

    fn sha256(&self) -> &str {
        self.artifact.sha256()
    }

    fn data_rows(&self) -> u64 {
        self.artifact.data_rows()
    }

    fn input_bytes(&self) -> u64 {
        self.artifact.input_bytes()
    }
}

pub fn writer_factory() -> Arc<dyn SpreadsheetWriterFactory> {
    Arc::new(SpreadsheetFactoryBridge)
}

pub fn document_processor() -> Arc<dyn SpreadsheetDocumentProcessor> {
    Arc::new(SpreadsheetFactoryBridge)
}
