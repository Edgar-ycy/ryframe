use super::modules::{Function, Modules};
use super::values::{edit_form_value, form_input, form_typescript_type, ts_default};
use super::{FieldIr, ResourceIr, ValueType, WidgetIr};

pub(super) fn render(modules: &mut Modules<'_>) {
    let resource = modules.resource;
    let fields = resource
        .fields
        .iter()
        .filter(|field| field.usage.create || field.usage.update)
        .collect::<Vec<_>>();
    let mut leaves = fields
        .chunks(24)
        .map(|fields| form_part(resource, fields))
        .collect::<Vec<_>>();
    if leaves.is_empty() {
        leaves.push(form_part(resource, &[]));
    }
    let pascal = &resource.pascal_name;
    let imports = format!(
        "import type {{ {pascal}Record }} from './api'\nimport type {{ {pascal}Form }} from './model'\n"
    );
    let edit_parameter = format!("record: {pascal}Record");
    let form_parameter = format!("form: Readonly<{pascal}Form>");
    let functions = [
        Function {
            name: "emptyForm",
            parameters: "",
            arguments: "",
            result: "",
            array: false,
        },
        Function {
            name: "editForm",
            parameters: &edit_parameter,
            arguments: "record",
            result: "",
            array: false,
        },
        Function {
            name: "createInput",
            parameters: &form_parameter,
            arguments: "form",
            result: "",
            array: false,
        },
        Function {
            name: "updateInput",
            parameters: &form_parameter,
            arguments: "form",
            result: "",
            array: false,
        },
    ];
    modules.tree("form", leaves, &functions, &imports);
    modules.add("model.ts", format!("import type {{ emptyForm }} from './form'\n\nexport type {pascal}Form = ReturnType<typeof emptyForm>\n"));
    query_defaults(modules);
}

fn form_part(resource: &ResourceIr, fields: &[&FieldIr]) -> String {
    let pascal = &resource.pascal_name;
    let mut output = format!(
        "import type {{\n  {pascal}CreateInput,\n  {pascal}UpdateInput,\n  {pascal}Record,\n}} from '../api'\n\ninterface FormPart {{\n"
    );
    for field in fields {
        output.push_str(&format!(
            "  {}: {}\n",
            field.name,
            form_typescript_type(field)
        ));
    }
    output.push_str("}\n");
    for mode in ["empty", "edit", "create", "update"] {
        let (name, parameter, result) = match mode {
            "empty" => ("emptyForm", String::new(), "FormPart".into()),
            "edit" => (
                "editForm",
                format!("record: {pascal}Record"),
                "FormPart".into(),
            ),
            "create" => (
                "createInput",
                "form: Readonly<FormPart>".into(),
                format!("Pick<{pascal}CreateInput, keyof FormPart & keyof {pascal}CreateInput>"),
            ),
            _ => (
                "updateInput",
                "form: Readonly<FormPart>".into(),
                format!("Pick<{pascal}UpdateInput, keyof FormPart & keyof {pascal}UpdateInput>"),
            ),
        };
        let properties = fields
            .iter()
            .filter(|field| match mode {
                "create" => field.usage.create,
                "update" => field.usage.update,
                _ => true,
            })
            .map(|field| {
                let value = match mode {
                    "empty" => ts_default(field).to_owned(),
                    "edit" => edit_form_value(field),
                    "create" => form_input(field, field.usage.create_optional),
                    _ => form_input(field, field.usage.update_optional),
                };
                format!("    {}: {value},\n", field.name)
            })
            .collect::<String>();
        let parameter = if properties.is_empty() && !parameter.is_empty() {
            format!("_{parameter}")
        } else {
            parameter
        };
        output.push_str(&format!("\nexport function {name}({parameter}): {result} {{\n  return {{\n{properties}  }}\n}}\n"));
    }
    output
}

fn query_defaults(modules: &mut Modules<'_>) {
    let fields = modules
        .resource
        .fields
        .iter()
        .filter(|field| {
            field.usage.filter
                && !matches!(field.widget, WidgetIr::Hidden)
                && field.value_type == ValueType::String
        })
        .collect::<Vec<_>>();
    let mut leaves = fields
        .chunks(100)
        .map(|fields| {
            let properties = fields
                .iter()
                .map(|field| format!("    {}: '' as const,\n", field.name))
                .collect::<String>();
            format!("export function queryDefaults() {{\n  return {{\n{properties}  }}\n}}\n")
        })
        .collect::<Vec<_>>();
    if leaves.is_empty() {
        leaves.push("export function queryDefaults() {\n  return {}\n}\n".into());
    }
    modules.tree(
        "query-defaults",
        leaves,
        &[Function {
            name: "queryDefaults",
            parameters: "",
            arguments: "",
            result: "",
            array: false,
        }],
        "",
    );
}
