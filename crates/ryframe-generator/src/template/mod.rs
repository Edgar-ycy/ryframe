pub mod catalog;
pub mod dto;
pub mod entity;
pub mod handler;
pub mod repository;
pub mod use_case;

use crate::schema::{ColumnInfo, TableInfo};

pub(crate) fn business_primary_keys(table: &TableInfo) -> Vec<&ColumnInfo> {
    let primary = table
        .indexes
        .iter()
        .find(|index| index.name == "PRIMARY")
        .expect("table schema is validated before rendering");
    primary
        .columns
        .iter()
        .filter(|name| name.as_str() != "tenant_id")
        .map(|name| {
            table
                .columns
                .iter()
                .find(|column| column.name == *name)
                .expect("PRIMARY column exists in table metadata")
        })
        .collect()
}

pub(crate) fn is_managed_column(column: &ColumnInfo) -> bool {
    column.is_primary_key
        || matches!(
            column.name.as_str(),
            "tenant_id" | "del_flag" | "created_at" | "updated_at" | "create_time" | "update_time"
        )
}

pub(crate) fn command_columns(table: &TableInfo) -> impl Iterator<Item = &ColumnInfo> {
    table
        .columns
        .iter()
        .filter(|column| !is_managed_column(column))
}

pub(crate) fn public_columns(table: &TableInfo) -> impl Iterator<Item = &ColumnInfo> {
    table.columns.iter().filter(|column| {
        !matches!(
            column.name.as_str(),
            "tenant_id" | "del_flag" | "updated_at" | "update_time"
        )
    })
}

pub(crate) fn chrono_import<'a>(columns: impl Iterator<Item = &'a ColumnInfo>) -> &'static str {
    if columns
        .into_iter()
        .any(|column| column.rust_type.contains("DateTime<Utc>"))
    {
        "use chrono::{DateTime, Utc};\n"
    } else {
        ""
    }
}

pub(crate) fn normal_value(column: &ColumnInfo) -> String {
    let value = if column.rust_type.contains("String") {
        "\"0\".to_string()"
    } else if column.rust_type.contains("bool") {
        "false"
    } else {
        "0"
    };
    optional_value(column, value)
}

pub(crate) fn deleted_value(column: &ColumnInfo) -> String {
    let value = if column.rust_type.contains("String") {
        "\"2\".to_string()"
    } else if column.rust_type.contains("bool") {
        "true"
    } else {
        "2"
    };
    optional_value(column, value)
}

fn optional_value(column: &ColumnInfo, value: &str) -> String {
    if column.rust_type.starts_with("Option<") {
        format!("Some({})", value)
    } else {
        value.into()
    }
}
