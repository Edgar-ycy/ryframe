use super::super::{FieldIr, IndexIr, ResourceIr, ValueType, rust_base_type};

pub(crate) fn command_type(field: &FieldIr, optional: bool) -> String {
    let value_type = rust_base_type(field.value_type);
    if optional || field.nullable {
        format!("Option<{value_type}>")
    } else {
        value_type.to_owned()
    }
}
pub(crate) fn unique_business_indexes(resource: &ResourceIr) -> impl Iterator<Item = &IndexIr> {
    resource.indexes.iter().filter(|index| {
        index.unique
            && index.fields != resource.primary_key
            && !business_index_fields(resource, index).is_empty()
            && business_index_fields(resource, index)
                .iter()
                .all(|field| !field.nullable)
    })
}

pub(crate) fn business_index_fields<'a>(
    resource: &'a ResourceIr,
    index: &IndexIr,
) -> Vec<&'a FieldIr> {
    index
        .fields
        .iter()
        .filter(|name| name.as_str() != "tenant_id")
        .map(|name| {
            resource
                .fields
                .iter()
                .find(|field| field.name == *name)
                .expect("索引字段已由 IR 校验")
        })
        .collect()
}

pub(crate) fn unique_method_name(index: &IndexIr) -> String {
    format!(
        "find_by_{}_for_update",
        index
            .fields
            .iter()
            .filter(|field| field.as_str() != "tenant_id")
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("_and_")
    )
}

pub(crate) fn method_arguments(fields: &[&FieldIr]) -> String {
    fields
        .iter()
        .map(|field| {
            let argument_type = if field.value_type == ValueType::String {
                "&str".to_owned()
            } else {
                rust_base_type(field.value_type).to_owned()
            };
            format!("        {}: {argument_type},\n", field.name)
        })
        .collect()
}

pub(crate) fn argument_expression(field: &FieldIr, expression: &str) -> String {
    if field.value_type == ValueType::String {
        format!("&{expression}")
    } else {
        expression.to_owned()
    }
}
