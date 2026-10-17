use super::values::{edit_form_value, form_input, ts_default};
use super::{FieldIr, ResourceIr, ValueType, WidgetIr};

pub(super) fn render_crud_resource(
    resource: &ResourceIr,
    form_fields: &[&FieldIr],
    output: &mut String,
) {
    let query_defaults = resource
        .fields
        .iter()
        .filter(|field| {
            field.usage.filter
                && !matches!(field.widget, WidgetIr::Hidden)
                && matches!(field.value_type, ValueType::String)
        })
        .map(|field| format!("      {}: '',\n", field.name))
        .collect::<String>();
    let empty_form = form_fields
        .iter()
        .map(|field| format!("      {}: {},\n", field.name, ts_default(field)))
        .collect::<String>();
    let edit_form = form_fields
        .iter()
        .map(|field| format!("      {}: {},\n", field.name, edit_form_value(field)))
        .collect::<String>();
    let create_input = resource
        .fields
        .iter()
        .filter(|field| field.usage.create)
        .map(|field| {
            format!(
                "      {}: {},\n",
                field.name,
                form_input(field, field.usage.create_optional)
            )
        })
        .collect::<String>();
    let update_input = resource
        .fields
        .iter()
        .filter(|field| field.usage.update)
        .map(|field| {
            format!(
                "      {}: {},\n",
                field.name,
                form_input(field, field.usage.update_optional)
            )
        })
        .collect::<String>();
    output.push_str(&definition(
        resource,
        &Mappings {
            query_defaults,
            empty_form,
            edit_form,
            create_input,
            update_input,
        },
    ));
}

pub(super) struct Mappings {
    pub query_defaults: String,
    pub empty_form: String,
    pub edit_form: String,
    pub create_input: String,
    pub update_input: String,
}

pub(super) fn definition(resource: &ResourceIr, mappings: &Mappings) -> String {
    let pascal = &resource.pascal_name;
    let record_id = resource
        .primary_key
        .iter()
        .rev()
        .find(|field| Some(field.as_str()) != resource.tenant_field.as_deref())
        .map(String::as_str)
        .unwrap_or("id");
    let record_label = resource
        .fields
        .iter()
        .find(|field| {
            field.name == "name"
                && field.value_type == ValueType::String
                && (field.usage.read || field.usage.list)
        })
        .or_else(|| {
            resource.fields.iter().find(|field| {
                field.value_type == ValueType::String && (field.usage.list || field.usage.read)
            })
        })
        .map(|field| field.name.as_str())
        .unwrap_or(record_id);
    format!(
        include_str!("../resource.ts.tpl"),
        name = resource.name,
        zh = resource.labels.zh_cn,
        en = resource.labels.en,
        pascal = pascal,
        query_defaults = mappings.query_defaults,
        empty_form = mappings.empty_form,
        edit_form = mappings.edit_form,
        create_input = mappings.create_input,
        update_input = mappings.update_input,
        record_id = record_id,
        record_label = record_label,
    )
}
