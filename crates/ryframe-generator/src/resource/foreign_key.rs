use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{ResourceError, ResourceIr, StorageKind};
use crate::import::{ForeignKeyAction, ForeignKeyMatch};

/// 物理同库约束；列名是数据库列名，不表示页面关系或自动联表。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForeignKeySpec {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
    pub update_rule: ForeignKeyAction,
    pub delete_rule: ForeignKeyAction,
    pub match_option: ForeignKeyMatch,
}

fn error(message: impl Into<String>) -> ResourceError {
    ResourceError::new(
        message,
        "声明同存储范围的完整受管外键；审阅约束、列序、类型和租户边界，本批未写入",
    )
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(crate) fn local(keys: &[ForeignKeySpec]) -> Result<(), ResourceError> {
    let mut names = BTreeSet::new();
    for key in keys {
        if !identifier(&key.name)
            || !identifier(&key.referenced_table)
            || !names.insert(key.name.to_ascii_lowercase())
            || key.columns.is_empty()
            || key.columns.len() != key.referenced_columns.len()
            || [&key.columns, &key.referenced_columns]
                .iter()
                .any(|columns| {
                    columns.iter().any(|column| !identifier(column))
                        || columns
                            .iter()
                            .map(|column| column.to_ascii_lowercase())
                            .collect::<BTreeSet<_>>()
                            .len()
                            != columns.len()
                })
        {
            return Err(error("外键名称、同库表名或有序列集合非法或重复"));
        }
        if key.match_option != ForeignKeyMatch::None
            || [key.update_rule, key.delete_rule].contains(&ForeignKeyAction::SetDefault)
        {
            return Err(error(
                "MySQL 物理外键只支持 MATCH NONE 和 CASCADE/RESTRICT/SET NULL/NO ACTION",
            ));
        }
    }
    Ok(())
}

fn validate_target(
    child: &ResourceIr,
    parent: &ResourceIr,
    key: &ForeignKeySpec,
) -> Result<(), ResourceError> {
    if child.storage != parent.storage
        || child.access.system_tenant_only != parent.access.system_tenant_only
    {
        return Err(error(format!("外键 {} 跨存储范围或租户作用域", key.name)));
    }
    let columns = |resource: &ResourceIr, fields: &[String]| {
        fields
            .iter()
            .map(|field| resource.column(field).to_owned())
            .collect::<Vec<_>>()
    };
    if !columns(child, &child.primary_key).starts_with(&key.columns)
        && !child
            .indexes
            .iter()
            .any(|index| columns(child, &index.fields).starts_with(&key.columns))
    {
        return Err(error(format!(
            "外键 {} 缺少同列序的显式本表索引；拒绝依赖数据库隐式补索引",
            key.name
        )));
    }
    if columns(parent, &parent.primary_key) != key.referenced_columns
        && !parent
            .indexes
            .iter()
            .any(|index| index.unique && columns(parent, &index.fields) == key.referenced_columns)
    {
        return Err(error(format!(
            "外键 {} 引用列序必须完整匹配主键或唯一索引",
            key.name
        )));
    }
    for (column, referenced) in key.columns.iter().zip(&key.referenced_columns) {
        let field = child
            .fields
            .iter()
            .find(|field| &field.column == column)
            .ok_or_else(|| error("外键本列不存在"))?;
        let target = parent
            .fields
            .iter()
            .find(|field| &field.column == referenced)
            .ok_or_else(|| error("外键引用列不存在"))?;
        let physical_type = super::schema_column_type(field);
        if physical_type == "JSON"
            || physical_type.contains("TEXT")
            || physical_type != super::schema_column_type(target)
            || (field.value_type == super::ValueType::String
                && child.column_collation(field) != parent.column_collation(target))
            || (!field.nullable
                && [key.update_rule, key.delete_rule].contains(&ForeignKeyAction::SetNull))
        {
            return Err(error(format!(
                "外键 {} 列类型、排序规则或 SET NULL 可空性不兼容",
                key.name
            )));
        }
    }
    if let Some(tenant) = child.tenant_field.as_deref() {
        let same_tenant = parent.tenant_field.as_deref().is_some_and(|target| {
            key.columns
                .iter()
                .zip(&key.referenced_columns)
                .any(|(column, referenced)| {
                    column == child.column(tenant) && referenced == parent.column(target)
                })
        });
        if !same_tenant {
            return Err(error(format!(
                "外键 {} 缺少同一 tenant_id 物理边界",
                key.name
            )));
        }
    }
    Ok(())
}

pub(crate) fn order(resources: &[ResourceIr]) -> Result<Vec<usize>, ResourceError> {
    if resources
        .iter()
        .all(|resource| resource.foreign_keys.is_empty())
    {
        return Ok((0..resources.len()).collect());
    }
    let mut names = BTreeSet::new();
    for resource in resources.iter().filter(|resource| {
        resource.storage == StorageKind::ControlRow && !resource.access.system_tenant_only
    }) {
        names.insert(format!("fk_{}_tenant", resource.name).to_ascii_lowercase());
    }
    let mut tables = BTreeSet::new();
    for resource in resources {
        local(&resource.foreign_keys)?;
        if !tables.insert(resource.table.to_ascii_lowercase()) {
            return Err(error("受管资源物理表重复"));
        }
        for key in &resource.foreign_keys {
            if !names.insert(key.name.to_ascii_lowercase()) {
                return Err(error("同一数据库范围的外键约束名重复"));
            }
            let parent = resources
                .iter()
                .find(|parent| parent.table == key.referenced_table)
                .ok_or_else(|| {
                    error(format!(
                        "外键 {} 的引用表不在当前完整受管资源集合",
                        key.name
                    ))
                })?;
            validate_target(resource, parent, key)?;
        }
    }
    topological(resources)
}

fn topological(resources: &[ResourceIr]) -> Result<Vec<usize>, ResourceError> {
    let tables = resources
        .iter()
        .enumerate()
        .map(|(index, resource)| (resource.table.as_str(), index))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut children = vec![BTreeSet::new(); resources.len()];
    let mut incoming = vec![0usize; resources.len()];
    for (child, resource) in resources.iter().enumerate() {
        for key in &resource.foreign_keys {
            let parent = *tables
                .get(key.referenced_table.as_str())
                .ok_or_else(|| error("外键目标未解析"))?;
            if children[parent].insert(child) {
                incoming[child] = incoming[child]
                    .checked_add(1)
                    .ok_or_else(|| error("外键依赖计数溢出"))?;
            }
        }
    }
    let mut ready = incoming
        .iter()
        .enumerate()
        .filter_map(|(index, count)| (*count == 0).then_some(index))
        .collect::<BTreeSet<_>>();
    let mut ordered = Vec::new();
    while let Some(parent) = ready.pop_first() {
        ordered.push(parent);
        for child in &children[parent] {
            incoming[*child] = incoming[*child]
                .checked_sub(1)
                .ok_or_else(|| error("外键依赖计数不一致"))?;
            if incoming[*child] == 0 {
                ready.insert(*child);
            }
        }
    }
    if ordered.len() != resources.len() {
        return Err(error(
            "物理外键依赖存在循环；当前不支持循环约束，本批未写入",
        ));
    }
    Ok(ordered)
}

pub(crate) fn action(value: ForeignKeyAction) -> &'static str {
    match value {
        ForeignKeyAction::Cascade => "CASCADE",
        ForeignKeyAction::Restrict => "RESTRICT",
        ForeignKeyAction::SetNull => "SET NULL",
        ForeignKeyAction::NoAction => "NO ACTION",
        ForeignKeyAction::SetDefault => "SET DEFAULT",
    }
}

pub(crate) fn ddl(key: &ForeignKeySpec) -> String {
    let columns = |columns: &[String]| {
        columns
            .iter()
            .map(|column| format!("`{column}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "  CONSTRAINT `{}` FOREIGN KEY ({}) REFERENCES `{}` ({}) ON UPDATE {} ON DELETE {}",
        key.name,
        columns(&key.columns),
        key.referenced_table,
        columns(&key.referenced_columns),
        action(key.update_rule),
        action(key.delete_rule)
    )
}

impl From<&crate::import::ForeignKeyInfo> for ForeignKeySpec {
    fn from(key: &crate::import::ForeignKeyInfo) -> Self {
        Self {
            name: key.name.clone(),
            columns: key.columns.clone(),
            referenced_table: key.referenced_table.clone(),
            referenced_columns: key.referenced_columns.clone(),
            update_rule: key.update_rule,
            delete_rule: key.delete_rule,
            match_option: key.match_option,
        }
    }
}

pub(crate) fn descriptor_literal(key: &ForeignKeySpec) -> String {
    format!(
        "ryframe_sdk::ResourceForeignKeyDescriptor {{ name: {:?}, columns: &{:?}, referenced_table: {:?}, referenced_columns: &{:?}, update_rule: ryframe_sdk::ResourceForeignKeyAction::{:?}, delete_rule: ryframe_sdk::ResourceForeignKeyAction::{:?}, match_option: ryframe_sdk::ResourceForeignKeyMatch::None }}",
        key.name,
        key.columns,
        key.referenced_table,
        key.referenced_columns,
        key.update_rule,
        key.delete_rule
    )
}
