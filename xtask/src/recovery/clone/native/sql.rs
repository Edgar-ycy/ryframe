use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

use regex::Regex;
use serde_json::Value;

use super::{catalog::Catalog, json};
use crate::Result;

pub(crate) const DATA_HEADER: &str = "-- ryframe-reference-data-v1";
const MAX_ROW_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SqlValue {
    Null,
    Bytes(Vec<u8>),
    Number(String),
    Text(String),
    Json(Value),
}

pub(crate) type Row = BTreeMap<String, SqlValue>;

impl SqlValue {
    pub(crate) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn integer(&self) -> Option<i64> {
        let Self::Number(value) = self else {
            return None;
        };
        value.parse::<i64>().ok()
    }

    pub(crate) fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }
}

pub(crate) fn for_each_row(
    path: &Path,
    catalog: &Catalog,
    only_table: Option<&str>,
    visit: impl FnMut(&str, &Row) -> Result<()>,
) -> Result<()> {
    let stream = File::open(path)
        .map_err(|error| format!("无法读取复制数据转储 {}：{error}", path.display()))?;
    for_each_reader(BufReader::new(stream), catalog, only_table, visit)
}

pub(crate) fn for_each_reader(
    mut reader: impl BufRead,
    catalog: &Catalog,
    only_table: Option<&str>,
    mut visit: impl FnMut(&str, &Row) -> Result<()>,
) -> Result<()> {
    if only_table.is_some_and(|table| !catalog.contains_key(table)) {
        return Err("SQL 过滤表不属于完整目录".into());
    }
    let header = bounded_line(&mut reader)?.ok_or("数据转储缺少规范头")?;
    if header != format!("{DATA_HEADER}\n") && header != format!("{DATA_HEADER}\r\n") {
        return Err("只接受现有工具规范化的数据转储".into());
    }
    while let Some(line) = bounded_line(&mut reader)? {
        let (table, row) = parse_row(line.trim_end_matches(['\r', '\n']), catalog)?;
        if only_table.is_none_or(|selected| selected == table) {
            visit(&table, &row)?;
        }
    }
    Ok(())
}

fn bounded_line(reader: &mut impl BufRead) -> Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8(line)?))
            };
        }
        let count = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |end| end + 1);
        if line
            .len()
            .checked_add(count)
            .is_none_or(|length| length > MAX_ROW_BYTES)
        {
            return Err("单行导出超过受限检查大小".into());
        }
        line.extend_from_slice(&buffer[..count]);
        let complete = buffer[count - 1] == b'\n';
        reader.consume(count);
        if complete {
            return Ok(Some(String::from_utf8(line)?));
        }
    }
}

pub(crate) fn parse_row(line: &str, catalog: &Catalog) -> Result<(String, Row)> {
    let structure = Regex::new(r"^INSERT INTO `([a-z0-9_-]+)` \((.*?)\) VALUES \((.*)\);$")?;
    let part = structure
        .captures(line)
        .ok_or("SQL 行结构无效或包含多语句")?;
    let table = &part[1];
    let columns = catalog.get(table).ok_or("SQL 引用了复制清单之外的表")?;
    validate_insert(&part[2])?;
    let names = Regex::new(r"`([a-z0-9_]+)`")?
        .captures_iter(&part[2])
        .map(|capture| capture[1].to_owned())
        .collect::<Vec<_>>();
    let values = literals(&part[3])?;
    if names.len() != values.len()
        || names.len() != names.iter().collect::<BTreeSet<_>>().len()
        || names.iter().cloned().collect::<BTreeSet<_>>()
            != columns.keys().cloned().collect::<BTreeSet<_>>()
    {
        return Err("SQL 必须完整且恰好包含当前表的全部列".into());
    }
    let mut row = Row::new();
    for (name, value) in names.into_iter().zip(values) {
        let value = if columns.get(&name).is_some_and(|kind| kind == "JSON") {
            match value {
                SqlValue::Null => SqlValue::Null,
                SqlValue::Text(text) => SqlValue::Json(
                    json::parse_fragment(&text, "SQL JSON")
                        .map_err(|_| "SQL JSON 字段无效、重复或包含非有限数字")?,
                ),
                _ => return Err("SQL JSON 列必须为字符串或 NULL".into()),
            }
        } else {
            value
        };
        row.insert(name, value);
    }
    Ok((table.to_owned(), row))
}

pub(crate) fn literals(source: &str) -> Result<Vec<SqlValue>> {
    let mut result = Vec::new();
    let mut position = 0;
    loop {
        skip_space(source, &mut position);
        if source[position..].starts_with("_binary") {
            position += "_binary".len();
            skip_space(source, &mut position);
        }
        if position >= source.len() {
            return Err("SQL 值缺失".into());
        }
        let (value, next) = if source[position..].starts_with('\'') {
            quoted(source, position)?
        } else {
            unsigned_literal(source, position)?
        };
        result.push(value);
        position = next;
        skip_space(source, &mut position);
        if position == source.len() {
            break;
        }
        if !source[position..].starts_with(',') || position + 1 == source.len() {
            return Err("SQL 列值分隔无效".into());
        }
        position += 1;
    }
    Ok(result)
}

fn validate_insert(columns: &str) -> Result<()> {
    let valid = Regex::new(r"^(?:`[a-z0-9_]+`)(?:, `[a-z0-9_]+`)*$")?;
    if valid.is_match(columns) {
        Ok(())
    } else {
        Err("备份 SQL 只能使用显式、唯一且规范的列名".into())
    }
}

fn quoted(source: &str, start: usize) -> Result<(SqlValue, usize)> {
    let mut output = String::new();
    let mut chars = source[start + 1..].char_indices().peekable();
    while let Some((offset, character)) = chars.next() {
        if character == '\'' {
            return Ok((SqlValue::Text(output), start + 1 + offset + 1));
        }
        if character != '\\' {
            output.push(character);
            continue;
        }
        let Some((_, escaped)) = chars.next() else {
            return Err("SQL 字符串包含未支持转义".into());
        };
        let value = match escaped {
            '0' => '\0',
            'b' => '\u{8}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'Z' => '\u{1a}',
            '\\' => '\\',
            '\'' => '\'',
            '"' => '"',
            '%' => {
                output.push('\\');
                '%'
            }
            '_' => {
                output.push('\\');
                '_'
            }
            _ => return Err("SQL 字符串包含未支持转义".into()),
        };
        output.push(value);
    }
    Err("SQL 字符串未结束".into())
}

fn unsigned_literal(source: &str, start: usize) -> Result<(SqlValue, usize)> {
    let pattern =
        Regex::new(r"^(?:NULL|0x[0-9a-fA-F]+|[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)")?;
    let matched = pattern.find(&source[start..]).ok_or("SQL 不是受限字面量")?;
    let token = matched.as_str();
    let value = match token {
        "NULL" => SqlValue::Null,
        _ if token.starts_with("0x") => {
            SqlValue::Bytes(hex::decode(&token[2..]).map_err(|_| "SQL 十六进制字面量无效")?)
        }
        _ => SqlValue::Number(token.to_owned()),
    };
    Ok((value, start + token.len()))
}

fn skip_space(source: &str, position: &mut usize) {
    while let Some(character) = source[*position..].chars().next() {
        if !character.is_whitespace() {
            break;
        }
        *position += character.len_utf8();
    }
}

pub(crate) fn all_columns(control: &Catalog, tenant: &Catalog, combined: bool) -> Catalog {
    let mut result = Catalog::new();
    if combined {
        result.extend(control.clone());
    }
    result.extend(tenant.clone());
    result
}

pub(crate) fn empty_counts(catalog: &Catalog) -> BTreeMap<String, u64> {
    catalog.keys().cloned().map(|table| (table, 0)).collect()
}
