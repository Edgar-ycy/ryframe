mod api;
mod application;
mod database;
mod frontend;
pub(super) mod migration;

use super::super::{FieldIr, IndexIr, ResourceIr, StorageKind, ValueType};

fn id_type(resource: &ResourceIr) -> &'static str {
    resource
        .fields
        .iter()
        .find(|field| field.name == "id")
        .expect("资源主键已经校验")
        .value_type
        .rust_base_type()
}

fn uses_filters(resource: &ResourceIr) -> bool {
    resource.access.owner_field.is_some()
        || resource
            .fields
            .iter()
            .any(|field| field.usage.filter && field.name != "tenant_id")
}

fn filter_lifetime(resource: &ResourceIr, lifetime: &str) -> String {
    let borrows = resource.access.owner_field.is_some()
        || resource.fields.iter().any(|field| {
            field.usage.filter && field.name != "tenant_id" && field.value_type == ValueType::String
        });
    if borrows {
        format!("<{lifetime}>")
    } else {
        String::new()
    }
}

fn system_tenant_guard(resource: &ResourceIr, module: &str) -> String {
    if resource.access.system_tenant_only {
        format!("        {module}::ensure_system_tenant_id(tenant_id)?;\n")
    } else {
        String::new()
    }
}

pub(super) fn api_dto(resource: &ResourceIr, header: &str) -> String {
    api::dto(resource, header)
}

pub(super) fn api_handler(resource: &ResourceIr, header: &str) -> String {
    api::handler(resource, header)
}

pub(super) fn api_openapi(resource: &ResourceIr, header: &str) -> String {
    api::openapi(resource, header)
}

pub(super) fn business_openapi(
    resource: &ResourceIr,
    header: &str,
) -> (String, Vec<(String, String)>) {
    api::business_openapi(resource, header)
}

pub(super) fn application_fake(resource: &ResourceIr, header: &str) -> String {
    application::fake(resource, header)
}

pub(super) fn application_model(resource: &ResourceIr, header: &str) -> String {
    application::model(resource, header)
}

pub(super) fn application_port(resource: &ResourceIr, header: &str) -> String {
    application::port(resource, header)
}

pub(super) fn application_service(resource: &ResourceIr, header: &str) -> String {
    application::service(resource, header)
}

pub(super) fn database_entity(resource: &ResourceIr, header: &str) -> String {
    database::entity(resource, header)
}

pub(super) fn database_repository(
    resource: &ResourceIr,
    resources: &[&ResourceIr],
    header: &str,
) -> String {
    database::repository(resource, resources, header)
}

pub(super) fn frontend_api(resource: &ResourceIr) -> String {
    frontend::api(resource)
}

pub(super) fn frontend_fields(resource: &ResourceIr) -> String {
    frontend::fields(resource)
}

pub(super) fn frontend_page(resource: &ResourceIr) -> String {
    frontend::page(resource)
}

pub(super) fn frontend_registration(resource: &ResourceIr) -> String {
    frontend::registration(resource)
}

pub(super) fn migration(resource: &ResourceIr, header: &str) -> String {
    migration::migration(resource, header)
}

pub(super) fn migration_name(resource: &ResourceIr) -> String {
    migration::migration_name(resource)
}

pub(super) fn rust_base_type(value_type: ValueType) -> &'static str {
    value_type.rust_base_type()
}

pub(super) fn column_variant(name: &str) -> String {
    crate::naming::to_pascal_case(name)
}

pub(super) fn uses_partial_text_filter(field: &FieldIr) -> bool {
    field.value_type == ValueType::String
        && field.enum_values.is_empty()
        && !field.usage.filter_exact
}

pub(super) fn rust_literal(value: &toml::Value, value_type: ValueType) -> String {
    match (value, value_type) {
        (toml::Value::String(value), ValueType::String) => format!("String::from({value:?})"),
        (toml::Value::String(value), ValueType::U64) => format!("{value}_u64"),
        (toml::Value::Integer(value), value_type) if value_type.integer_bounds().is_some() => {
            format!("{value}_{}", value_type.rust_base_type())
        }
        (toml::Value::Boolean(value), ValueType::Bool) => value.to_string(),
        _ => unreachable!("literal 类型已经在 Resource IR 中严格校验"),
    }
}

pub(super) fn sql_literal(value: &toml::Value) -> String {
    match value {
        toml::Value::String(value)
            if value
                .chars()
                .any(|character| character == '\\' || character.is_control()) =>
        {
            format!("_utf8mb4 X'{}'", hex::encode(value.as_bytes()))
        }
        toml::Value::String(value) => format!("'{}'", value.replace('\'', "''")),
        toml::Value::Integer(value) => value.to_string(),
        toml::Value::Boolean(value) => i32::from(*value).to_string(),
        _ => unreachable!("SQL literal 类型已经在 Resource IR 中严格校验"),
    }
}
