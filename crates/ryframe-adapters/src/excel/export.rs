use std::{borrow::Cow, io::Read, path::Path};

use rust_xlsxwriter::{Color, DataValidation, Format, Workbook, Worksheet};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::{Builder, TempPath};

use ryframe_kernel::{AppError, AppResult};
/// Excel 导出工具
pub struct ExcelExporter;

/// XLSX 工作表在保留一行表头后可容纳的数据行硬上限。
pub const XLSX_MAX_DATA_ROWS: u64 = 1_048_575;

/// 一次增量写入完成后的累计进度。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExcelBatchProgress {
    pub batch_rows: u64,
    pub total_rows: u64,
    pub total_input_bytes: u64,
}

/// 自动清理的 XLSX 临时产物。
///
/// 产物持有期间路径有效；对象上传完成或失败后只需释放该值，临时文件会自动删除。
pub struct ExcelArtifact {
    path: TempPath,
    size: u64,
    sha256: String,
    data_rows: u64,
    input_bytes: u64,
}

impl ExcelArtifact {
    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }

    pub const fn size(&self) -> u64 {
        self.size
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub const fn data_rows(&self) -> u64 {
        self.data_rows
    }

    pub const fn input_bytes(&self) -> u64 {
        self.input_bytes
    }
}

/// 以常量内存模式逐批写入单个 XLSX 工作表。
///
/// 行必须按顺序追加。写入过程只保留当前行，工作表 XML 和最终 XLSX 都落到临时文件，
/// 不会构造全量业务行或完整字节副本。
pub struct IncrementalExcelWriter<'headers> {
    workbook: Workbook,
    headers: &'headers [(&'headers str, &'headers str)],
    row_limit: u64,
    data_rows: u64,
    input_bytes: u64,
}

impl<'headers> IncrementalExcelWriter<'headers> {
    pub fn new(
        sheet_name: &str,
        headers: &'headers [(&'headers str, &'headers str)],
    ) -> AppResult<Self> {
        Self::with_row_limit(sheet_name, headers, XLSX_MAX_DATA_ROWS)
    }

    /// 创建带业务行数上限的增量写入器，上限不得超过 XLSX 硬限制。
    pub fn with_row_limit(
        sheet_name: &str,
        headers: &'headers [(&'headers str, &'headers str)],
        row_limit: u64,
    ) -> AppResult<Self> {
        if row_limit == 0 || row_limit > XLSX_MAX_DATA_ROWS {
            return Err(AppError::Validation(format!(
                "Excel 行数上限必须在 1 到 {XLSX_MAX_DATA_ROWS} 之间"
            )));
        }
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet_with_constant_memory();
        ExcelExporter::set_sheet_name(worksheet, sheet_name)?;
        ExcelExporter::write_headers(worksheet, headers)?;
        ExcelExporter::auto_width(worksheet, headers.len())?;

        Ok(Self {
            workbook,
            headers,
            row_limit,
            data_rows: 0,
            input_bytes: 0,
        })
    }

    pub const fn data_rows(&self) -> u64 {
        self.data_rows
    }

    pub const fn input_bytes(&self) -> u64 {
        self.input_bytes
    }

    /// 消费一批行并直接追加到工作表，不保留批次副本。
    pub fn append_rows<I>(&mut self, rows: I) -> AppResult<ExcelBatchProgress>
    where
        I: IntoIterator,
        I::Item: Serialize,
    {
        let mut batch_rows = 0u64;
        for item in rows {
            let next_row = self
                .data_rows
                .checked_add(1)
                .ok_or_else(|| AppError::PayloadTooLarge("Excel 导出行数累计溢出".to_owned()))?;
            if next_row > self.row_limit {
                return Err(AppError::ExportRowLimitExceeded {
                    matched_rows: next_row,
                    limit: self.row_limit,
                });
            }

            let value = serde_json::to_value(&item)
                .map_err(|error| AppError::Internal(format!("序列化数据失败: {error}")))?;
            let object = value
                .as_object()
                .ok_or_else(|| AppError::Internal("Excel 导出行必须序列化为对象".to_owned()))?;
            let worksheet = self
                .workbook
                .worksheet_from_index(0)
                .map_err(|error| AppError::Internal(format!("读取导出工作表失败: {error}")))?;

            for (column, (field, _)) in self.headers.iter().enumerate() {
                let Some(value) = object.get(*field) else {
                    continue;
                };
                let text = ExcelExporter::value_to_text(value);
                let value_bytes = u64::try_from(text.len()).map_err(|_| {
                    AppError::PayloadTooLarge("Excel 单元格内容长度溢出".to_owned())
                })?;
                self.input_bytes = self.input_bytes.checked_add(value_bytes).ok_or_else(|| {
                    AppError::PayloadTooLarge("Excel 导出输入字节累计溢出".to_owned())
                })?;
                worksheet
                    .write_string(next_row as u32, column as u16, text.as_ref())
                    .map_err(|error| AppError::Internal(format!("写入数据失败: {error}")))?;
            }

            self.data_rows = next_row;
            batch_rows += 1;
        }

        Ok(ExcelBatchProgress {
            batch_rows,
            total_rows: self.data_rows,
            total_input_bytes: self.input_bytes,
        })
    }

    /// 将工作簿保存为自动清理的临时 XLSX 文件。
    pub fn finish(mut self) -> AppResult<ExcelArtifact> {
        let output = Builder::new()
            .prefix("ryframe-export-")
            .suffix(".xlsx")
            .tempfile()
            .map_err(|error| AppError::Internal(format!("创建 Excel 临时文件失败: {error}")))?
            .into_temp_path();
        let output_path: &Path = output.as_ref();
        self.workbook
            .save(output_path)
            .map_err(|error| AppError::Internal(format!("生成 Excel 失败: {error}")))?;
        let size = std::fs::metadata(output_path)
            .map_err(|error| AppError::Internal(format!("读取 Excel 文件大小失败: {error}")))?
            .len();
        let sha256 = hash_file(output_path)?;

        Ok(ExcelArtifact {
            path: output,
            size,
            sha256,
            data_rows: self.data_rows,
            input_bytes: self.input_bytes,
        })
    }
}

fn hash_file(path: &Path) -> AppResult<String> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| AppError::Internal(format!("打开 Excel 临时文件失败: {error}")))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| AppError::Internal(format!("读取 Excel 临时文件失败: {error}")))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

impl ExcelExporter {
    /// 导出数据到 Excel 字节数组
    pub fn export_to_bytes<T: Serialize>(
        data: &[T],
        sheet_name: &str,
        headers: &[(&str, &str)],
    ) -> AppResult<Vec<u8>> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();

        Self::set_sheet_name(worksheet, sheet_name)?;
        Self::write_headers(worksheet, headers)?;
        Self::write_data_rows(worksheet, data, headers)?;
        Self::auto_width(worksheet, headers.len())?;

        let buf = workbook
            .save_to_buffer()
            .map_err(|e| AppError::Internal(format!("生成 Excel 失败: {}", e)))?;

        Ok(buf)
    }

    /// 导出模板（仅表头）
    pub fn export_template(sheet_name: &str, headers: &[(&str, &str)]) -> AppResult<Vec<u8>> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();

        Self::set_sheet_name(worksheet, sheet_name)?;
        Self::write_headers(worksheet, headers)?;
        Self::auto_width(worksheet, headers.len())?;

        let buf = workbook
            .save_to_buffer()
            .map_err(|e| AppError::Internal(format!("生成模板失败: {}", e)))?;

        Ok(buf)
    }

    /// 导出带参考值工作表的模板，供使用者复制稳定业务值而不是数据库 ID。
    pub fn export_template_with_reference(
        sheet_name: &str,
        headers: &[(&str, &str)],
        reference_sheet_name: &str,
        reference_header: &str,
        reference_values: &[String],
    ) -> AppResult<Vec<u8>> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();
        Self::set_sheet_name(worksheet, sheet_name)?;
        Self::write_headers(worksheet, headers)?;
        Self::auto_width(worksheet, headers.len())?;
        worksheet
            .set_column_width(headers.len().saturating_sub(1) as u16, 40.0)
            .map_err(|error| AppError::Internal(format!("设置模板列宽失败: {error}")))?;
        worksheet
            .set_freeze_panes(1, 0)
            .map_err(|error| AppError::Internal(format!("冻结模板表头失败: {error}")))?;

        let reference_sheet = workbook.add_worksheet();
        Self::set_sheet_name(reference_sheet, reference_sheet_name)?;
        let reference_headers = [("reference_value", reference_header)];
        Self::write_headers(reference_sheet, &reference_headers)?;
        for (row, value) in reference_values.iter().enumerate() {
            reference_sheet
                .write_string((row + 1) as u32, 0, value)
                .map_err(|error| AppError::Internal(format!("写入模板参考值失败: {error}")))?;
        }
        reference_sheet
            .set_column_width(0, 50.0)
            .map_err(|error| AppError::Internal(format!("设置参考工作表列宽失败: {error}")))?;
        reference_sheet
            .set_freeze_panes(1, 0)
            .map_err(|error| AppError::Internal(format!("冻结参考工作表表头失败: {error}")))?;

        if !reference_values.is_empty() {
            workbook
                .define_name(
                    "AvailableDepartmentPaths",
                    &format!(
                        "='{reference_sheet_name}'!$A$2:$A${}",
                        reference_values.len() + 1
                    ),
                )
                .map_err(|error| AppError::Internal(format!("定义模板参考范围失败: {error}")))?;
            let validation = DataValidation::new()
                .allow_list_formula("=AvailableDepartmentPaths".into())
                .set_input_title("选择部门完整路径")
                .and_then(|value| {
                    value.set_input_message("请从下拉列表选择，或从“可用部门”工作表复制完整路径。")
                })
                .and_then(|value| value.set_error_title("部门完整路径无效"))
                .and_then(|value| value.set_error_message("请选择当前模板列出的可用部门完整路径。"))
                .map_err(|error| AppError::Internal(format!("创建模板下拉校验失败: {error}")))?;
            workbook
                .worksheet_from_name(sheet_name)
                .map_err(|error| AppError::Internal(format!("读取模板工作表失败: {error}")))?
                .add_data_validation(
                    1,
                    headers.len().saturating_sub(1) as u16,
                    20_000,
                    headers.len().saturating_sub(1) as u16,
                    &validation,
                )
                .map_err(|error| AppError::Internal(format!("添加模板下拉校验失败: {error}")))?;
        }

        workbook
            .save_to_buffer()
            .map_err(|error| AppError::Internal(format!("生成模板失败: {error}")))
    }

    // ── 内部辅助方法 ──

    fn header_format() -> Format {
        Format::new()
            .set_bold()
            .set_background_color(Color::Blue)
            .set_font_color(Color::White)
    }

    fn set_sheet_name(ws: &mut Worksheet, name: &str) -> AppResult<()> {
        ws.set_name(name)
            .map_err(|error| AppError::Internal(format!("设置工作表名称失败: {error}")))?;
        Ok(())
    }

    fn write_headers(ws: &mut Worksheet, headers: &[(&str, &str)]) -> AppResult<()> {
        let fmt = Self::header_format();
        for (col, (_, title)) in headers.iter().enumerate() {
            ws.write_string_with_format(0, col as u16, *title, &fmt)
                .map_err(|e| AppError::Internal(format!("写入表头失败: {}", e)))?;
        }
        Ok(())
    }

    fn write_data_rows<T: Serialize>(
        ws: &mut Worksheet,
        data: &[T],
        headers: &[(&str, &str)],
    ) -> AppResult<()> {
        for (row, item) in data.iter().enumerate() {
            let val = serde_json::to_value(item)
                .map_err(|e| AppError::Internal(format!("序列化数据失败: {}", e)))?;

            if let Some(obj) = val.as_object() {
                for (col, (field, _)) in headers.iter().enumerate() {
                    if let Some(v) = obj.get(*field) {
                        let s = Self::value_to_str(v);
                        ws.write_string((row + 1) as u32, col as u16, &s)
                            .map_err(|e| AppError::Internal(format!("写入数据失败: {}", e)))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn auto_width(ws: &mut Worksheet, cols: usize) -> AppResult<()> {
        for i in 0..cols {
            ws.set_column_width(i as u16, 15.0)
                .map_err(|e| AppError::Internal(format!("设置列宽失败: {}", e)))?;
        }
        Ok(())
    }

    fn value_to_text(v: &serde_json::Value) -> Cow<'_, str> {
        match v {
            serde_json::Value::String(value) => Cow::Borrowed(value),
            serde_json::Value::Number(value) => Cow::Owned(value.to_string()),
            serde_json::Value::Bool(value) => Cow::Owned(value.to_string()),
            serde_json::Value::Null => Cow::Borrowed(""),
            other => Cow::Owned(other.to_string()),
        }
    }

    pub fn value_to_str(v: &serde_json::Value) -> String {
        Self::value_to_text(v).into_owned()
    }
}
