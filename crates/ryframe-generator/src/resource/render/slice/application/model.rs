use super::super::{ResourceIr, rust_base_type};
use super::command_type;

pub(crate) fn model(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let mut output = format!("{header}use ryframe_kernel::ValidatedPageQuery;\n\n");
    output.push_str("#[derive(Clone, Debug, PartialEq)]\n");
    output.push_str(&format!("pub struct {pascal}Record {{\n"));
    for field in &resource.fields {
        output.push_str(&format!("    pub {}: {},\n", field.name, field.rust_type));
    }
    output.push_str("}\n\n");

    if !resource.relations.is_empty() {
        output.push_str("#[derive(Clone, Debug, PartialEq)]\n");
        output.push_str(&format!("pub struct {pascal}Detail {{\n"));
        output.push_str(&format!("    pub record: {pascal}Record,\n"));
        for relation in &resource.relations {
            output.push_str(&format!(
                "    pub {}: Option<crate::generated::{}::{}Record>,\n",
                relation.name, relation.target_resource, relation.target_pascal_name
            ));
        }
        output.push_str("}\n\n");
    }

    output.push_str("#[derive(Clone, Debug)]\n");
    output.push_str(&format!("pub struct Create{pascal}Command {{\n"));
    for field in resource.fields.iter().filter(|field| field.usage.create) {
        output.push_str(&format!(
            "    pub {}: {},\n",
            field.name,
            command_type(field, field.usage.create_optional)
        ));
    }
    output.push_str("}\n\n");

    output.push_str("#[derive(Clone, Debug)]\n");
    output.push_str(&format!("pub struct Update{pascal}Command {{\n"));
    for field in resource.fields.iter().filter(|field| field.usage.update) {
        output.push_str(&format!(
            "    pub {}: {},\n",
            field.name,
            command_type(field, field.usage.update_optional)
        ));
    }
    output.push_str("}\n\n");

    output.push_str("#[derive(Clone, Debug)]\n");
    output.push_str(&format!("pub struct {pascal}ListParams {{\n"));
    output.push_str("    pub page: ValidatedPageQuery,\n");
    for field in resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
    {
        output.push_str(&format!(
            "    pub {}: Option<{}>,\n",
            field.name,
            rust_base_type(field.value_type)
        ));
    }
    output.push_str("}\n\n");

    output.push_str("#[derive(Clone, Copy, Debug, Default)]\n");
    output.push_str(&format!("pub struct {pascal}Filter<'a> {{\n"));
    for field in resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
    {
        let value_type = if matches!(field.value_type, super::super::ValueType::String) {
            "&'a str".to_owned()
        } else {
            rust_base_type(field.value_type).to_owned()
        };
        output.push_str(&format!("    pub {}: Option<{value_type}>,\n", field.name));
    }
    output.push_str("}\n");
    output
}
