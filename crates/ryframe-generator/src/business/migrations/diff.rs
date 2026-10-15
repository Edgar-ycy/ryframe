use std::collections::BTreeMap;

use crate::resource::{FieldIr, column_definition, schema_column_type};
use crate::{ResourceError, ResourceIr, ValueType};

/// 只生成可以明确表达的向前结构变更；需要回填或可能损失数据的操作仅返回审阅说明。
pub(super) fn statements(
    before: &ResourceIr,
    after: &ResourceIr,
) -> Result<Vec<String>, ResourceError> {
    let mut review = Vec::new();
    if before.table_collation() != after.table_collation() {
        review.push("表排序规则变化，需要审阅比较语义、索引和存量数据".into());
    }
    if before.table != after.table
        || before.storage != after.storage
        || before.id_generation != after.id_generation
        || primary_columns(before) != primary_columns(after)
    {
        review.push("表名、存储范围、ID 策略或物理主键变化，需要单独审阅数据迁移".to_owned());
    }
    let old = before
        .fields
        .iter()
        .map(|field| (field.column.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let new = after
        .fields
        .iter()
        .map(|field| (field.column.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let old_order = before
        .fields
        .iter()
        .filter(|field| new.contains_key(field.column.as_str()))
        .map(|field| &field.column)
        .collect::<Vec<_>>();
    let new_order = after
        .fields
        .iter()
        .filter(|field| old.contains_key(field.column.as_str()))
        .map(|field| &field.column)
        .collect::<Vec<_>>();
    if old_order != new_order {
        review.push("已有列顺序发生变化，需要单独审阅结构调整".into());
    }
    for column in old.keys().filter(|column| !new.contains_key(**column)) {
        review.push(format!(
            "列 {column} 被删除或重命名，先设计数据保留、回填及切换步骤"
        ));
    }
    let mut clauses = Vec::new();
    for (index, field) in after.fields.iter().enumerate() {
        if let Some(previous) = old.get(field.column.as_str()) {
            if previous.value_type == ValueType::String
                && field.value_type == ValueType::String
                && before.column_collation(previous) != after.column_collation(field)
            {
                review.push(format!(
                    "列 {} 排序规则变化，需要审阅比较语义、索引和存量数据",
                    field.column
                ));
            }
            if column_definition(before, previous) != column_definition(after, field) {
                if !safe_type(previous, field) || (previous.nullable && !field.nullable) {
                    review.push(format!(
                        "列 {} 类型 {} → {}、可空 {} → {}，涉及收窄类型、改变语义或收紧非空约束，先核对现有数据并回填",
                        field.column, schema_column_type(previous), schema_column_type(field), previous.nullable, field.nullable
                    ));
                } else {
                    clauses.push(format!(
                        "MODIFY COLUMN {}",
                        column_definition(after, field).trim()
                    ));
                }
            }
        } else if !field.nullable && field.default.is_none() {
            review.push(format!(
                "新增非空列 {} 没有显式默认值，须先新增可空列并回填，再审阅非空约束",
                field.column
            ));
        } else {
            let position = if index == 0 {
                "FIRST".into()
            } else {
                format!("AFTER `{}`", after.fields[index - 1].column)
            };
            clauses.push(format!(
                "ADD COLUMN {} {position}",
                column_definition(after, field).trim()
            ));
        }
    }
    indexes(before, after, &mut clauses, &mut review);
    if !review.is_empty() {
        return Err(ResourceError::new(format!("模型结构变更需要人工审阅，本批未写入：\n- {}", review.join("\n- ")),
            "保留历史迁移；先明确数据回填或重命名方案，使用可空列分阶段演进，不通过重写初始迁移处理已有数据库").with_resource(&after.name));
    }
    if clauses.is_empty() {
        return Err(ResourceError::new(
            "结构指纹已变化，但没有可确认的增量 DDL，本批未写入",
            "核对模型、历史描述及索引，不能跳过结构差异",
        )
        .with_resource(&after.name));
    }
    Ok(vec![format!(
        "ALTER TABLE `{}`\n  {}",
        after.table,
        clauses.join(",\n  ")
    )])
}

fn safe_type(before: &FieldIr, after: &FieldIr) -> bool {
    let old = schema_column_type(before);
    let new = schema_column_type(after);
    old == new
        || matches!((crate::resource::physical_integer_bounds(before), crate::resource::physical_integer_bounds(after)), (Some((old_min, old_max)), Some((new_min, new_max))) if new_min <= old_min && new_max >= old_max)
        || (before.value_type == ValueType::String
            && after.value_type == ValueType::String
            && matches!((varchar_length(&old), varchar_length(&new)), (Some(old), Some(new)) if new >= old))
}

fn varchar_length(value: &str) -> Option<u32> {
    value
        .strip_prefix("VARCHAR(")?
        .strip_suffix(')')?
        .parse()
        .ok()
}

fn primary_columns(resource: &ResourceIr) -> Vec<&str> {
    resource
        .primary_key
        .iter()
        .map(|field| resource.column(field))
        .collect()
}

fn indexes(
    before: &ResourceIr,
    after: &ResourceIr,
    clauses: &mut Vec<String>,
    review: &mut Vec<String>,
) {
    let old = index_definitions(before);
    let new = index_definitions(after);
    for (name, definition) in &old {
        if new.get(name) != Some(definition) {
            review.push(format!(
                "索引 {name} 被删除或改变，需单独审阅查询与约束影响"
            ));
        }
    }
    for (name, definition) in new {
        if !old.contains_key(&name) {
            clauses.push(format!("ADD {definition}"));
        }
    }
}

fn index_definitions(resource: &ResourceIr) -> BTreeMap<String, String> {
    resource
        .indexes
        .iter()
        .map(|index| {
            let columns = index
                .fields
                .iter()
                .map(|field| format!("`{}`", resource.column(field)))
                .collect::<Vec<_>>()
                .join(", ");
            (
                index.name.clone(),
                format!(
                    "{}KEY `{}` ({columns})",
                    if index.unique { "UNIQUE " } else { "" },
                    index.name
                ),
            )
        })
        .collect()
}
