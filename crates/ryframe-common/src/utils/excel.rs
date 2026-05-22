use calamine::{open_workbook_auto, Data, Reader, Xlsx};
use crate::{AppError, AppResult};
use rust_xlsxwriter::{Color, Format, Workbook, Worksheet};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::io::Cursor;

/// Excel 导入工具
pub struct ExcelImporter;

impl ExcelImporter {
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

    /// 获取目标工作表范围
    fn range_from_sheet_names<R, RS>(
        workbook: &mut R,
        sheet_name: Option<&str>,
    ) -> AppResult<calamine::Range<Data>>
    where
        R: Reader<RS>,
        R::Error: std::fmt::Display, RS: std::io::Read + std::io::Seek
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
    fn parse_range<T: DeserializeOwned>(
        range: &calamine::Range<Data>,
    ) -> AppResult<Vec<T>> {
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
                .filter_map(|(i, h)| {
                    row.get(i).map(|c| (h.clone(), c.to_string()))
                })
                .collect();

            let json = serde_json::to_value(&map)
                .map_err(|e| AppError::Internal(format!("序列化失败: {}", e)))?;

            let value = serde_json::from_value(json)
                .map_err(|e| AppError::Validation(format!("解析第 {} 行失败: {}", row_no, e)))?;

            results.push(value);
        }

        Ok(results)
    }
}

/// Excel 导出工具
pub struct ExcelExporter;

impl ExcelExporter {
    /// 导出数据到 Excel 字节数组
    pub fn export_to_bytes<T: Serialize>(
        data: &[T],
        _sheet_name: &str,
        headers: &[(&str, &str)],
    ) -> AppResult<Vec<u8>> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();

        Self::write_headers(worksheet, headers)?;
        Self::write_data_rows(worksheet, data, headers)?;
        Self::auto_width(worksheet, headers.len())?;

        let buf = workbook
            .save_to_buffer()
            .map_err(|e| AppError::Internal(format!("生成 Excel 失败: {}", e)))?;

        Ok(buf)
    }

    /// 导出模板（仅表头）
    pub fn export_template(
        _sheet_name: &str,
        headers: &[(&str, &str)],
    ) -> AppResult<Vec<u8>> {
        let mut workbook = Workbook::new();
        let worksheet = workbook.add_worksheet();

        Self::write_headers(worksheet, headers)?;
        Self::auto_width(worksheet, headers.len())?;

        let buf = workbook
            .save_to_buffer()
            .map_err(|e| AppError::Internal(format!("生成模板失败: {}", e)))?;

        Ok(buf)
    }

    // ── 内部辅助方法 ──

    fn header_format() -> Format {
        Format::new()
            .set_bold()
            .set_background_color(Color::Blue)
            .set_font_color(Color::White)
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

    fn value_to_str(v: &serde_json::Value) -> String {
        match v {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        }
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
