use std::{collections::BTreeSet, fmt::Write};

use super::super::{ForeignKeySpec, ResourceIr};

pub(super) fn dependencies(resource: &ResourceIr) -> Vec<&str> {
    resource
        .foreign_keys
        .iter()
        .map(|key| key.referenced_table.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn descriptor(resource: &ResourceIr, path: &str) -> String {
    resource.foreign_keys.iter().map(|key| format!(
        "{path} {{ name: {:?}, columns: &{:?}, referenced_table: {:?}, referenced_columns: &{:?} }}",
        key.name, key.columns, key.referenced_table, key.referenced_columns,
    )).collect::<Vec<_>>().join(", ")
}

pub(super) fn manifest(resource: &ResourceIr) -> Vec<serde_json::Value> {
    resource
        .foreign_keys
        .iter()
        .map(|key| {
            serde_json::json!({
                "name": key.name, "columns": key.columns, "referenced_table": key.referenced_table,
                "referenced_columns": key.referenced_columns,
            })
        })
        .collect()
}

pub(super) fn canonical(output: &mut String, keys: &[ForeignKeySpec]) {
    let mut keys = keys.iter().collect::<Vec<_>>();
    keys.sort_by_key(|key| key.name.to_ascii_lowercase());
    output.push_str("]|checks=[]|foreign_keys=[");
    for key in keys {
        for (position, (column, referenced)) in
            key.columns.iter().zip(&key.referenced_columns).enumerate()
        {
            write!(
                output,
                "{:?}:{:?}:{}:local:{:?}:{:?}:{:?}:{:?};",
                key.name,
                column,
                position + 1,
                key.referenced_table,
                referenced,
                super::super::foreign_key::action(key.update_rule),
                super::super::foreign_key::action(key.delete_rule)
            )
            .expect("写入 String 不会失败");
        }
    }
    output.push(']');
}
