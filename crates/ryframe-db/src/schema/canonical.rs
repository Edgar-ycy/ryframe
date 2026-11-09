use sea_orm::DbErr;

use super::{
    CheckMetadata, ColumnMetadata, ConstraintMetadata, ForeignKeySchemaRow, IndexMetadata,
    TableMetadata, difference::split_unquoted,
};

/// 只解码现有 v2 表描述，不解释 CREATE/ALTER，也不修改规范化规则。
pub(super) struct Canonical {
    pub table: TableMetadata,
    pub columns: Vec<ColumnMetadata>,
    pub indexes: Vec<IndexMetadata>,
    pub constraints: Vec<ConstraintMetadata>,
    pub checks: Vec<CheckMetadata>,
    pub foreign_keys: Vec<ForeignKeySchemaRow>,
}

impl Canonical {
    pub fn decode(table_name: &str, value: &str) -> Result<Self, DbErr> {
        let parts = split_unquoted(value, '|');
        let keys = [
            "table",
            "engine",
            "charset",
            "collation",
            "columns",
            "indexes",
            "constraints",
            "checks",
            "foreign_keys",
        ];
        if parts.len() != keys.len() + 1 || parts[0] != "v2" {
            return Err(invalid());
        }
        let values = keys
            .iter()
            .zip(&parts[1..])
            .map(|(key, part)| part.strip_prefix(&format!("{key}=")).ok_or_else(invalid))
            .collect::<Result<Vec<_>, _>>()?;
        let table = TableMetadata {
            table_name: text(values[0])?,
            engine: text(values[1])?,
            character_set_name: text(values[2])?,
            table_collation: text(values[3])?,
        };
        if table.table_name != table_name {
            return Err(invalid());
        }
        let result = Self {
            table,
            columns: rows(values[4], column)?,
            indexes: rows(values[5], index)?,
            constraints: rows(values[6], constraint)?,
            checks: rows(values[7], check)?,
            foreign_keys: rows(values[8], foreign_key)?,
        };
        if result.columns.is_empty() || !result.unique() || result.render() != value {
            return Err(invalid());
        }
        Ok(result)
    }

    fn unique(&self) -> bool {
        fn distinct<T: Ord>(values: impl Iterator<Item = T>) -> bool {
            let mut seen = std::collections::BTreeSet::new();
            values.into_iter().all(|value| seen.insert(value))
        }
        distinct(self.columns.iter().map(|v| &v.column_name))
            && distinct(self.indexes.iter().map(|v| (&v.index_name, v.seq_in_index)))
            && distinct(self.constraints.iter().map(|v| &v.constraint_name))
            && distinct(self.checks.iter().map(|v| &v.constraint_name))
            && distinct(
                self.foreign_keys
                    .iter()
                    .map(|v| (&v.constraint_name, v.ordinal_position)),
            )
    }

    fn render(&self) -> String {
        // 共用现有 renderer 做无损校验；不接受不规范的整数、Option 或转义形式。
        super::render_table_schema(
            self.table.clone(),
            self.columns.clone(),
            self.indexes.clone(),
            self.constraints.clone(),
            self.checks.clone(),
            self.foreign_keys.clone(),
        )
    }
}

fn rows<T>(value: &str, decode: fn(&[&str]) -> Result<T, DbErr>) -> Result<Vec<T>, DbErr> {
    let body = value
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(invalid)?;
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let body = body.strip_suffix(';').ok_or_else(invalid)?;
    split_unquoted(body, ';')
        .into_iter()
        .map(|row| decode(&split_unquoted(row, ':')))
        .collect()
}

fn column(v: &[&str]) -> Result<ColumnMetadata, DbErr> {
    if v.len() != 9 {
        return Err(invalid());
    }
    Ok(ColumnMetadata {
        column_name: text(v[0])?,
        column_type: text(v[1])?,
        is_nullable: text(v[2])?,
        character_set_name: optional(v[3], text)?,
        collation_name: optional(v[4], text)?,
        column_key: text(v[5])?,
        column_default: optional(v[6], text)?,
        extra: text(v[7])?,
        generation_expression: text(v[8])?,
    })
}

fn index(v: &[&str]) -> Result<IndexMetadata, DbErr> {
    if v.len() != 7 {
        return Err(invalid());
    }
    Ok(IndexMetadata {
        index_name: text(v[0])?,
        column_name: text(v[1])?,
        seq_in_index: integer(v[2])?,
        non_unique: integer(v[3])?,
        index_type: text(v[4])?,
        sub_part: optional(v[5], integer)?,
        is_visible: text(v[6])?,
    })
}

fn constraint(v: &[&str]) -> Result<ConstraintMetadata, DbErr> {
    if v.len() != 3 {
        return Err(invalid());
    }
    Ok(ConstraintMetadata {
        constraint_name: text(v[0])?,
        constraint_type: text(v[1])?,
        enforced: text(v[2])?,
    })
}

fn check(v: &[&str]) -> Result<CheckMetadata, DbErr> {
    if v.len() != 2 {
        return Err(invalid());
    }
    Ok(CheckMetadata {
        constraint_name: text(v[0])?,
        check_clause: text(v[1])?,
    })
}

fn foreign_key(v: &[&str]) -> Result<ForeignKeySchemaRow, DbErr> {
    if v.len() != 8 || v[3] != "local" {
        return Err(invalid());
    }
    Ok(ForeignKeySchemaRow {
        constraint_name: text(v[0])?,
        column_name: text(v[1])?,
        ordinal_position: integer(v[2])?,
        current_schema: String::new(),
        referenced_table_schema: String::new(),
        referenced_table_name: text(v[4])?,
        referenced_column_name: text(v[5])?,
        update_rule: text(v[6])?,
        delete_rule: text(v[7])?,
    })
}

fn optional<T>(value: &str, decode: fn(&str) -> Result<T, DbErr>) -> Result<Option<T>, DbErr> {
    if value == "None" {
        return Ok(None);
    }
    value
        .strip_prefix("Some(")
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(invalid)
        .and_then(decode)
        .map(Some)
}

fn integer(value: &str) -> Result<i64, DbErr> {
    value.parse().map_err(|_| invalid())
}

fn text(value: &str) -> Result<String, DbErr> {
    let body = value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(invalid)?;
    let mut chars = body.chars();
    let mut result = String::new();
    while let Some(c) = chars.next() {
        if c == '"' {
            return Err(invalid());
        }
        if c != '\\' {
            result.push(c);
            continue;
        }
        result.push(match chars.next().ok_or_else(invalid)? {
            '\\' => '\\',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            'u' => {
                if chars.next() != Some('{') {
                    return Err(invalid());
                }
                let mut hex = String::new();
                loop {
                    match chars.next().ok_or_else(invalid)? {
                        '}' => break,
                        digit if digit.is_ascii_hexdigit() => hex.push(digit),
                        _ => return Err(invalid()),
                    }
                }
                char::from_u32(u32::from_str_radix(&hex, 16).map_err(|_| invalid())?)
                    .ok_or_else(invalid)?
            }
            _ => return Err(invalid()),
        });
    }
    Ok(result)
}

fn invalid() -> DbErr {
    DbErr::Custom("表结构 v2 描述无法完整解码；核对迁移声明，尚未生成初始化 SQL".into())
}
