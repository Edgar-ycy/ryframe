//! 按旧冻结导出验证器的物理引用规则检查同一遍 SQL 扫描。
use std::{
    borrow::Cow,
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

use serde_json::Value;

use super::{Row, SqlValue, is_physical_key, replace_logical};
use crate::Result;

type Table = BTreeMap<char, String>;

const CASE_FOLDING: &str =
    include_str!("../../../../../assets/unicode-case-folding-15.1.0/CaseFolding-15.1.0.txt");
const FOLD_ERROR: &str = "固定 Unicode 15.1.0 折叠表无效";
const PHYSICAL_ERROR: &str = "数据含未解决的来源物理 scope、库或端点引用";
static FOLDING: LazyLock<std::result::Result<Table, &'static str>> =
    LazyLock::new(|| parse_table(CASE_FOLDING));

pub(in crate::recovery::clone::native) struct Check<'a> {
    forbidden: Vec<String>,
    logical: Vec<&'a str>,
    table: &'static Table,
}

impl<'a> Check<'a> {
    pub(in crate::recovery::clone::native) fn new(
        forbidden: &[String],
        logical: &'a BTreeSet<String>,
    ) -> Result<Self> {
        let table = FOLDING.as_ref().map_err(|error| *error)?;
        let forbidden = forbidden
            .iter()
            .map(|token| fold(token, table).into_owned())
            .collect();
        let mut logical = logical.iter().map(String::as_str).collect::<Vec<_>>();
        logical.sort_by_key(|tenant| Reverse(tenant.len()));
        Ok(Self {
            forbidden,
            logical,
            table,
        })
    }

    pub(in crate::recovery::clone::native) fn row(&self, row: &Row) -> Result<()> {
        for (key, value) in row {
            self.text(key, true)?;
            self.sql(value, !is_physical_key(key))?;
        }
        Ok(())
    }

    fn sql(&self, value: &SqlValue, allow_logical: bool) -> Result<()> {
        match value {
            SqlValue::Bytes(bytes) => self.text(
                std::str::from_utf8(bytes).map_err(|_| PHYSICAL_ERROR)?,
                allow_logical,
            ),
            SqlValue::Text(text) => self.text(text, allow_logical),
            SqlValue::Json(value) => self.json(value, allow_logical),
            SqlValue::Null | SqlValue::Number(_) => Ok(()),
        }
    }

    fn json(&self, value: &Value, allow_logical: bool) -> Result<()> {
        match value {
            Value::String(text) => self.text(text, allow_logical),
            Value::Array(values) => {
                for value in values {
                    self.json(value, allow_logical)?;
                }
                Ok(())
            }
            Value::Object(fields) => {
                for (key, value) in fields {
                    self.text(key, allow_logical)?;
                    // 旧规则由当前字段名决定其值的例外，数组本身则沿用父级选择。
                    self.json(value, !is_physical_key(key))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn text(&self, value: &str, allow_logical: bool) -> Result<()> {
        let mut examined = Cow::Borrowed(value);
        if allow_logical {
            for tenant in &self.logical {
                if examined.contains(*tenant) {
                    examined = Cow::Owned(replace_logical(&examined, tenant));
                }
            }
        }
        let folded = fold(&examined, self.table);
        if self.forbidden.iter().any(|token| folded.contains(token)) {
            return Err(PHYSICAL_ERROR.into());
        }
        Ok(())
    }
}

fn fold<'a>(value: &'a str, table: &Table) -> Cow<'a, str> {
    if value.is_ascii() {
        if value.bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Cow::Owned(value.to_ascii_lowercase());
        }
        return Cow::Borrowed(value);
    }
    let mut output = String::with_capacity(value.len());
    for point in value.chars() {
        if let Some(mapped) = table.get(&point) {
            output.push_str(mapped);
        } else {
            output.push(point);
        }
    }
    Cow::Owned(output)
}

fn parse_table(source: &str) -> std::result::Result<Table, &'static str> {
    let mut table = BTreeMap::new();
    for line in source.lines() {
        let data = line.split_once('#').map_or(line, |(data, _)| data).trim();
        if data.is_empty() {
            continue;
        }
        let mut fields = data.split(';').map(str::trim);
        let point = fields.next().ok_or(FOLD_ERROR)?;
        let status = fields.next().ok_or(FOLD_ERROR)?;
        let mapping = fields.next().ok_or(FOLD_ERROR)?;
        if fields.next() != Some("") || fields.next().is_some() {
            return Err(FOLD_ERROR);
        }
        match status {
            "S" | "T" => continue,
            "C" | "F" => {}
            _ => return Err(FOLD_ERROR),
        }
        let mapped = mapping
            .split_ascii_whitespace()
            .map(scalar)
            .collect::<std::result::Result<String, _>>()?;
        if mapped.is_empty() || table.insert(scalar(point)?, mapped).is_some() {
            return Err(FOLD_ERROR);
        }
    }
    if table.len() != 1530 {
        return Err(FOLD_ERROR);
    }
    Ok(table)
}

fn scalar(value: &str) -> std::result::Result<char, &'static str> {
    u32::from_str_radix(value, 16)
        .ok()
        .and_then(char::from_u32)
        .ok_or(FOLD_ERROR)
}

#[cfg(test)]
#[path = "../../../../../tests/internal/export_native/frozen_physical.rs"]
mod tests;
