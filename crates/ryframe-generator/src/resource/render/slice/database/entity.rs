use super::super::{ResourceIr, ValueType};

pub(crate) fn entity(resource: &ResourceIr, header: &str) -> String {
    let mut output = format!(
        "{header}use sea_orm::entity::prelude::*;\n\n#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]\n#[sea_orm(table_name = {:?})]\npub struct Model {{\n",
        resource.table
    );
    // SeaORM 按字段声明顺序生成复合主键元组；只调整实体，DDL 仍保留物理列顺序。
    let keys = resource.primary_key.iter().map(|key| {
        resource
            .fields
            .iter()
            .find(|field| &field.name == key)
            .expect("主键已校验")
    });
    let other_fields = resource
        .fields
        .iter()
        .filter(|field| !resource.primary_key.contains(&field.name));
    for field in keys.chain(other_fields) {
        if resource.primary_key.contains(&field.name) {
            // ORM 不支持复合自增主键；租户插入由 Repository 显式省略 ID 并回读。
            let auto_increment = resource.primary_key.len() == 1
                && field.name == "id"
                && resource.id_generation == crate::IdGeneration::AutoIncrement;
            output.push_str(&format!(
                "    #[sea_orm(primary_key, auto_increment = {auto_increment})]\n"
            ));
        }
        if field.column != field.name {
            output.push_str(&format!(
                "    #[sea_orm(column_name = {:?})]\n",
                field.column
            ));
        }
        output.push_str(&format!("    pub {}: {},\n", field.name, field.rust_type));
    }
    output.push_str("}\n\n");
    output.push_str(&soft_delete_constants(resource));
    output.push_str(
        "#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]\npub enum Relation {}\n\nimpl ActiveModelBehavior for ActiveModel {}\n",
    );
    output
}

fn soft_delete_constants(resource: &ResourceIr) -> String {
    let Some(soft_delete) = &resource.soft_delete else {
        return String::new();
    };
    let field = resource
        .fields
        .iter()
        .find(|field| field.name == soft_delete.field)
        .expect("软删字段引用已经在 IR 中校验");
    let (constant_type, active, deleted) = match field.value_type {
        ValueType::String => (
            "&str",
            soft_delete
                .active
                .as_str()
                .expect("string 软删值已校验")
                .to_owned(),
            soft_delete
                .deleted
                .as_str()
                .expect("string 软删值已校验")
                .to_owned(),
        ),
        ValueType::U64 => (
            "u64",
            unsigned_value(&soft_delete.active),
            unsigned_value(&soft_delete.deleted),
        ),
        value_type if value_type.integer_bounds().is_some() => (
            value_type.rust_base_type(),
            soft_delete
                .active
                .as_integer()
                .expect("整数软删值已校验")
                .to_string(),
            soft_delete
                .deleted
                .as_integer()
                .expect("整数软删值已校验")
                .to_string(),
        ),
        ValueType::Bool => (
            "bool",
            soft_delete
                .active
                .as_bool()
                .expect("bool 软删值已校验")
                .to_string(),
            soft_delete
                .deleted
                .as_bool()
                .expect("bool 软删值已校验")
                .to_string(),
        ),
        _ => unreachable!("软删字段类型已经在 IR 中严格校验"),
    };
    let active = if field.value_type == ValueType::String {
        format!("{active:?}")
    } else {
        active
    };
    let deleted = if field.value_type == ValueType::String {
        format!("{deleted:?}")
    } else {
        deleted
    };
    format!(
        "pub const SOFT_DELETE_ACTIVE: {constant_type} = {active};\npub const SOFT_DELETE_DELETED: {constant_type} = {deleted};\n\n"
    )
}

fn unsigned_value(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value) => value.clone(),
        toml::Value::Integer(value) => value.to_string(),
        _ => unreachable!("u64 软删值已校验"),
    }
}
