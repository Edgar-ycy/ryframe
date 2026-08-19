use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    io::Cursor,
    path::Path,
};

use calamine::{Data, Reader, Xlsx, open_workbook_auto};
use rust_xlsxwriter::{Color, DataValidation, Format, Workbook, Worksheet};
use serde::{Serialize, de::DeserializeOwned};
use tempfile::{Builder, TempPath};

use ryframe_kernel::{AppError, AppResult};

/// Excel 导入工具
pub struct ExcelImporter;

/// 保留 Excel 原始行号的逐行解析结果。
pub struct ExcelImportRow<T> {
    pub row_number: usize,
    pub value: Result<T, String>,
}

impl ExcelImporter {
    /// 校验字节内容确实是包含工作表的 XLSX 工作簿，不解析业务行。
    pub fn validate_xlsx(bytes: &[u8]) -> AppResult<()> {
        let cursor = Cursor::new(bytes);
        let mut workbook = Xlsx::new(cursor)
            .map_err(|error| AppError::Validation(format!("文件内容不是有效的 XLSX: {error}")))?;
        Self::range_from_sheet_names(&mut workbook, None)
            .map_err(|error| AppError::Validation(format!("XLSX 工作表无效: {error}")))?;
        Ok(())
    }

    /// 严格校验目标工作表的表头，列名、列数和顺序都必须与契约一致。
    pub fn validate_headers_from_bytes(
        bytes: &[u8],
        sheet_name: Option<&str>,
        expected_headers: &[(&str, &str)],
    ) -> AppResult<()> {
        let cursor = Cursor::new(bytes);
        let mut workbook = Xlsx::new(cursor)
            .map_err(|error| AppError::Validation(format!("文件内容不是有效的 XLSX: {error}")))?;
        let range = Self::range_from_sheet_names(&mut workbook, sheet_name)
            .map_err(|error| AppError::Validation(format!("XLSX 工作表无效: {error}")))?;
        let actual_headers = range
            .rows()
            .next()
            .ok_or_else(|| AppError::Validation("Excel 工作表缺少表头".into()))?
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        let blank_columns = actual_headers
            .iter()
            .enumerate()
            .filter(|(_, header)| header.is_empty())
            .map(|(index, _)| (index + 1).to_string())
            .collect::<Vec<_>>();
        if !blank_columns.is_empty() {
            return Err(AppError::Validation(format!(
                "Excel 表头存在空白列：第 {} 列",
                blank_columns.join("、")
            )));
        }

        let mut seen = HashSet::with_capacity(actual_headers.len());
        let duplicate_headers = actual_headers
            .iter()
            .filter(|header| !seen.insert((*header).clone()))
            .cloned()
            .collect::<Vec<_>>();
        if !duplicate_headers.is_empty() {
            return Err(AppError::Validation(format!(
                "Excel 表头存在重复列：{}",
                duplicate_headers.join("、")
            )));
        }

        let expected_titles = expected_headers
            .iter()
            .map(|(_, title)| (*title).to_owned())
            .collect::<Vec<_>>();
        if actual_headers == expected_titles {
            return Ok(());
        }

        let expected_set = expected_titles.iter().collect::<HashSet<_>>();
        let actual_set = actual_headers.iter().collect::<HashSet<_>>();
        let unknown_headers = actual_headers
            .iter()
            .filter(|header| !expected_set.contains(header))
            .cloned()
            .collect::<Vec<_>>();
        let missing_headers = expected_titles
            .iter()
            .filter(|header| !actual_set.contains(header))
            .cloned()
            .collect::<Vec<_>>();

        let mut reasons = Vec::new();
        if !unknown_headers.is_empty() {
            reasons.push(format!("存在未知列：{}", unknown_headers.join("、")));
        }
        if !missing_headers.is_empty() {
            reasons.push(format!("缺少必需列：{}", missing_headers.join("、")));
        }
        if unknown_headers.is_empty()
            && missing_headers.is_empty()
            && actual_headers.len() == expected_titles.len()
        {
            reasons.push("列顺序不正确".into());
        }
        if reasons.is_empty() {
            reasons.push(format!(
                "列数不正确，期望 {} 列，实际 {} 列",
                expected_titles.len(),
                actual_headers.len()
            ));
        }

        Err(AppError::Validation(format!(
            "Excel 表头不符合导入模板：{}。请按以下顺序保留列：{}",
            reasons.join("；"),
            expected_titles.join("、")
        )))
    }

    /// 从文件读取 Excel 数据
    pub fn read_from_file<P: AsRef<std::path::Path>, T: DeserializeOwned>(
        path: P,
        sheet_name: Option<&str>,
    ) -> AppResult<Vec<T>> {
        let mut workbook = open_workbook_auto(path)
            .map_err(|e| AppError::Internal(format!("打开 Excel 文件失败: {}", e)))?;

        let range = Self::range_from_sheet_names(&mut workbook, sheet_name)?;
        Self::parse_range(&range)
    }

    /// 从字节读取 Excel 数据
    pub fn read_from_bytes<T: DeserializeOwned>(
        bytes: &[u8],
        sheet_name: Option<&str>,
    ) -> AppResult<Vec<T>> {
        let cursor = Cursor::new(bytes);
        let mut workbook = Xlsx::new(cursor)
            .map_err(|e| AppError::Internal(format!("解析 Excel 数据失败: {}", e)))?;

        let range = Self::range_from_sheet_names(&mut workbook, sheet_name)?;
        Self::parse_range(&range)
    }

    /// 从首个工作表逐行解析，同时保留无法反序列化的行供异步导入报告。
    pub fn read_rows_from_bytes<T: DeserializeOwned>(
        bytes: &[u8],
        sheet_name: Option<&str>,
    ) -> AppResult<Vec<ExcelImportRow<T>>> {
        let cursor = Cursor::new(bytes);
        let mut workbook = Xlsx::new(cursor)
            .map_err(|error| AppError::Validation(format!("解析 Excel 数据失败: {error}")))?;
        let range = Self::range_from_sheet_names(&mut workbook, sheet_name)?;
        Self::parse_rows(&range)
    }

    /// 获取目标工作表范围
    fn range_from_sheet_names<R, RS>(
        workbook: &mut R,
        sheet_name: Option<&str>,
    ) -> AppResult<calamine::Range<Data>>
    where
        R: Reader<RS>,
        R::Error: std::fmt::Display,
        RS: std::io::Read + std::io::Seek,
    {
        let name = match sheet_name {
            Some(n) => n.to_string(),
            None => {
                let sheets = workbook.sheet_names();
                if sheets.is_empty() {
                    return Err(AppError::Validation("Excel 文件没有工作表".into()));
                }
                sheets[0].clone()
            }
        };

        workbook
            .worksheet_range(&name)
            .map_err(|e| AppError::Internal(format!("读取工作表失败: {}", e)))
    }

    /// 解析工作表数据
    fn parse_range<T: DeserializeOwned>(range: &calamine::Range<Data>) -> AppResult<Vec<T>> {
        Self::parse_rows(range)?
            .into_iter()
            .map(|row| row.value.map_err(AppError::Validation))
            .collect()
    }

    fn parse_rows<T: DeserializeOwned>(
        range: &calamine::Range<Data>,
    ) -> AppResult<Vec<ExcelImportRow<T>>> {
        let mut results = Vec::new();
        let mut headers = Vec::new();
        let mut row_no = 0usize;

        for row in range.rows() {
            row_no += 1;

            if row_no == 1 {
                headers = row.iter().map(|c| c.to_string()).collect();
                continue;
            }

            if row.iter().all(|c| matches!(c, Data::Empty)) {
                continue;
            }

            let map: HashMap<String, String> = headers
                .iter()
                .enumerate()
                .filter_map(|(i, h)| row.get(i).map(|c| (h.clone(), c.to_string())))
                .collect();

            let json = serde_json::to_value(&map)
                .map_err(|e| AppError::Internal(format!("序列化失败: {}", e)))?;

            let value = serde_json::from_value(json)
                .map_err(|error| format!("解析第 {row_no} 行失败: {error}"));

            results.push(ExcelImportRow {
                row_number: row_no,
                value,
            });
        }

        Ok(results)
    }
}

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
    data_rows: u64,
    input_bytes: u64,
}

impl<'headers> IncrementalExcelWriter<'headers> {
    pub fn new(
        sheet_name: &str,
        headers: &'headers [(&'headers str, &'headers str)],
    ) -> AppResult<Self> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet_with_constant_memory();
        ExcelExporter::set_sheet_name(worksheet, sheet_name)?;
        ExcelExporter::write_headers(worksheet, headers)?;
        ExcelExporter::auto_width(worksheet, headers.len())?;

        Ok(Self {
            workbook,
            headers,
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
            if next_row > XLSX_MAX_DATA_ROWS {
                return Err(AppError::ExportRowLimitExceeded {
                    matched_rows: next_row,
                    limit: XLSX_MAX_DATA_ROWS,
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

        Ok(ExcelArtifact {
            path: output,
            size,
            data_rows: self.data_rows,
            input_bytes: self.input_bytes,
        })
    }
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

/// Excel 导入导出辅助宏
#[macro_export]
macro_rules! define_excel_mapping {
    ($ty:ident, [$(($field:expr, $title:expr)),+ $(,)?]) => {
        impl $ty {
            pub fn excel_headers() -> &'static [(&'static str, &'static str)] {
                &[$(($field, $title)),+]
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use calamine::{Reader, open_workbook_auto};
    use serde::Serialize;

    use super::{IncrementalExcelWriter, XLSX_MAX_DATA_ROWS};
    use ryframe_kernel::AppError;

    const HEADERS: &[(&str, &str)] = &[("id", "编号"), ("name", "名称")];

    #[derive(Serialize)]
    struct Row {
        id: u64,
        name: &'static str,
    }

    #[test]
    fn incremental_writer_consumes_batches_and_removes_artifact_on_drop() {
        let mut writer = IncrementalExcelWriter::new("测试", HEADERS).expect("创建写入器");
        let first = writer
            .append_rows([Row { id: 1, name: "甲" }])
            .expect("写入首批");
        let second = writer
            .append_rows([Row { id: 2, name: "乙" }])
            .expect("写入次批");

        assert_eq!(first.batch_rows, 1);
        assert_eq!(second.total_rows, 2);
        assert_eq!(second.total_input_bytes, 8);

        let artifact = writer.finish().expect("完成工作簿");
        assert_eq!(artifact.data_rows(), 2);
        assert_eq!(artifact.input_bytes(), 8);
        assert!(artifact.size() > 0);
        let artifact_path = artifact.path().to_path_buf();
        let mut workbook = open_workbook_auto(&artifact_path).expect("打开工作簿");
        let range = workbook.worksheet_range("测试").expect("读取工作表");
        assert_eq!(range.height(), 3);
        assert_eq!(
            range.get_value((2, 1)).map(ToString::to_string),
            Some("乙".into())
        );

        drop(workbook);
        drop(artifact);
        assert!(!artifact_path.exists());
    }

    #[test]
    fn incremental_writer_rejects_rows_past_xlsx_limit() {
        let mut writer = IncrementalExcelWriter::new("测试", HEADERS).expect("创建写入器");
        writer.data_rows = XLSX_MAX_DATA_ROWS;

        let error = writer
            .append_rows([Row { id: 1, name: "甲" }])
            .expect_err("超过 XLSX 行上限必须失败");
        assert!(matches!(
            error,
            AppError::ExportRowLimitExceeded {
                matched_rows: 1_048_576,
                limit: XLSX_MAX_DATA_ROWS
            }
        ));
    }
}
