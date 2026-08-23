use super::super::{ResourceIr, ValueType};

pub(crate) fn entity(resource: &ResourceIr, header: &str) -> String {
    let mut output = format!(
        "{header}use sea_orm::entity::prelude::*;\n\n#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]\n#[sea_orm(table_name = {:?})]\npub struct Model {{\n",
        resource.table
    );
    for field in &resource.fields {
        if resource.primary_key.contains(&field.name) {
            output.push_str("    #[sea_orm(primary_key, auto_increment = false)]\n");
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
        ValueType::I32 => (
            "i32",
            soft_delete
                .active
                .as_integer()
                .expect("i32 软删值已校验")
                .to_string(),
            soft_delete
                .deleted
                .as_integer()
                .expect("i32 软删值已校验")
                .to_string(),
        ),
        ValueType::I64 => (
            "i64",
            soft_delete
                .active
                .as_integer()
                .expect("i64 软删值已校验")
                .to_string(),
            soft_delete
                .deleted
                .as_integer()
                .expect("i64 软删值已校验")
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
