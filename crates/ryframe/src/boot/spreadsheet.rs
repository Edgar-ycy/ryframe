use std::sync::Arc;

use ryframe_adapters::excel::{
    ExcelArtifact, ExcelExporter, ExcelImporter, IncrementalExcelWriter,
};
use ryframe_application::{
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

#[cfg(test)]
mod tests {
    use ryframe_kernel::AppError;

    use super::*;

    #[test]
    fn creates_streaming_artifact_without_buffering_rows() {
        let mut writer = writer_factory()
            .create("测试", &[("id", "编号")])
            .expect("应创建表格写入器");
        let mut rows = std::iter::empty::<SpreadsheetRow>();
        let progress = writer.append_rows(&mut rows).expect("空批次应可写入");
        assert_eq!(progress.total_rows, 0);

        let artifact = writer.finish().expect("应生成表格制品");
        assert!(artifact.path().is_file());
        assert!(artifact.size() > 0);
        assert_eq!(artifact.data_rows(), 0);
    }

    #[tokio::test]
    async fn rejects_invalid_xlsx_source() {
        let error = document_processor()
            .validate_source(b"not-an-xlsx".to_vec(), &[("username", "用户名")])
            .await
            .expect_err("无效 XLSX 必须在上传前被拒绝");
        assert!(matches!(error, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn exports_and_reads_typed_rows_through_document_port() {
        let mut row = SpreadsheetRow::Object(Default::default());
        let SpreadsheetRow::Object(fields) = &mut row else {
            unreachable!("测试行必须是对象")
        };
        fields.insert("id".into(), SpreadsheetRow::String("42".into()));
        let processor = document_processor();
        let bytes = processor
            .export_rows(vec![row], "测试", &[("id", "编号")])
            .await
            .expect("应生成测试表格");
        let rows = processor
            .read_rows(bytes, &[("id", "编号")])
            .await
            .expect("应读回测试表格");

        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]
                .value
                .as_ref()
                .expect("测试行应有效")
                .get("编号")
                .and_then(SpreadsheetRow::as_str),
            Some("42")
        );
    }
}
