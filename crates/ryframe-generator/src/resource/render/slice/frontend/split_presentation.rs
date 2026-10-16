use super::modules::{Function, Modules};
use super::presentation::{column_properties, form_properties, query_properties};
use super::values::{push_typescript_object, ts_enum_value};
use super::{FieldIr, ResourceIr, WidgetIr, has_date_column};

pub(super) fn render(modules: &mut Modules<'_>) {
    options(modules);
    for kind in ["query", "columns", "form-fields"] {
        let resource = modules.resource;
        let fields = resource
            .fields
            .iter()
            .filter(|field| match kind {
                "query" => field.usage.filter && !matches!(field.widget, WidgetIr::Hidden),
                "columns" => field.usage.list,
                _ => field.usage.create || field.usage.update,
            })
            .collect::<Vec<_>>();
        let mut leaves = fields
            .chunks(16)
            .map(|fields| presentation_part(resource, kind, fields))
            .collect::<Vec<_>>();
        if leaves.is_empty() {
            leaves.push(presentation_part(resource, kind, &[]));
        }
        let (name, model, flat_type) = descriptor(kind);
        let pascal = &resource.pascal_name;
        let model_path = if model == "Form" { "model" } else { "api" };
        let imports = format!(
            "import type {{ {flat_type} }} from '@/components/business/flat-crud'\nimport type {{ {pascal}{model} }} from './{model_path}'\nimport type {{ Translate }} from './common'\n"
        );
        let date = kind == "columns" && has_date_column(resource);
        let parameters = if date {
            "translate: Translate, formatDate: (value: string) => string"
        } else {
            "translate: Translate"
        };
        let arguments = if date {
            "translate, formatDate"
        } else {
            "translate"
        };
        let result = format!(": readonly {flat_type}<{pascal}{model}>[]");
        modules.tree(
            kind,
            leaves,
            &[Function {
                name,
                parameters,
                arguments,
                result: &result,
                array: true,
            }],
            &imports,
        );
    }
}

fn descriptor(kind: &str) -> (&'static str, &'static str, &'static str) {
    match kind {
        "query" => ("queryFields", "Query", "FlatCrudQueryField"),
        "columns" => ("columns", "Record", "FlatCrudColumn"),
        _ => ("formFields", "Form", "FlatCrudFormField"),
    }
}

fn presentation_part(resource: &ResourceIr, kind: &str, fields: &[&FieldIr]) -> String {
    let (name, model, flat_type) = descriptor(kind);
    let pascal = &resource.pascal_name;
    let model_path = if model == "Form" { "model" } else { "api" };
    let mut output = format!(
        "import type {{ {flat_type} }} from '@/components/business/flat-crud'\nimport type {{ {pascal}{model} }} from '../{model_path}'\nimport type {{ Translate }} from '../common'\n"
    );
    for field in fields.iter().filter(|field| !field.enum_values.is_empty()) {
        output.push_str(&format!(
            "import {{ options as {}Options }} from '../options/{}'\n",
            field.name, field.name
        ));
    }
    let mut body = String::new();
    for field in fields {
        let mut properties = match kind {
            "query" => query_properties(field),
            "columns" => column_properties(field),
            _ => form_properties(field),
        };
        if !field.enum_values.is_empty() {
            for property in &mut properties {
                if property.starts_with("options:") {
                    property.push_str("(translate)");
                }
            }
        }
        push_typescript_object(&mut body, &properties);
    }
    let translate = if fields.is_empty() {
        "_translate"
    } else {
        "translate"
    };
    let date_parameter = if kind == "columns" && has_date_column(resource) {
        let date = if body.contains("format: formatDate") {
            "formatDate"
        } else {
            "_formatDate"
        };
        format!(", {date}: (value: string) => string")
    } else {
        String::new()
    };
    output.push_str(&format!("\nexport function {name}({translate}: Translate{date_parameter}): readonly {flat_type}<{pascal}{model}>[] {{\n  return [\n{body}  ]\n}}\n"));
    output
}

fn options(modules: &mut Modules<'_>) {
    let resource = modules.resource;
    for field in resource
        .fields
        .iter()
        .filter(|field| !field.enum_values.is_empty())
    {
        let entries = field.enum_values.iter().collect::<Vec<_>>();
        let leaves = entries.chunks(40).map(|entries| {
            let mut body = String::new();
            for (value, labels) in entries {
                push_typescript_object(&mut body, &[
                    format!("label: translate({:?}, {:?})", labels.zh_cn, labels.en),
                    format!("value: {}", ts_enum_value(value, field)),
                ]);
            }
            format!("import type {{ FlatCrudOption }} from '@/components/business/flat-crud'\nimport type {{ Translate }} from '../../common'\n\nexport function options(translate: Translate): readonly FlatCrudOption[] {{\n  return [\n{body}  ]\n}}\n")
        }).collect();
        let imports = "import type { FlatCrudOption } from '@/components/business/flat-crud'\nimport type { Translate } from '../common'\n";
        modules.tree(
            &format!("options/{}", field.name),
            leaves,
            &[Function {
                name: "options",
                parameters: "translate: Translate",
                arguments: "translate",
                result: ": readonly FlatCrudOption[]",
                array: true,
            }],
            imports,
        );
    }
}
