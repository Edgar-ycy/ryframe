use std::{
    collections::{HashMap, HashSet},
    io::{Cursor, Read, Seek},
    path::Path,
};

use calamine::{Data, Reader, Xlsx, open_workbook_auto};
use serde::de::DeserializeOwned;

use ryframe_kernel::{AppError, AppResult};

mod export;

pub use export::{
    ExcelArtifact, ExcelBatchProgress, ExcelExporter, IncrementalExcelWriter, XLSX_MAX_DATA_ROWS,
};

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
    pub fn read_from_file<P: AsRef<Path>, T: DeserializeOwned>(
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
        RS: Read + Seek,
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
