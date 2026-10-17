use super::resource::render_crud_resource;
use super::values::{form_widget, push_typescript_object, ts_enum_value};
use super::{FieldIr, ResourceIr, ValueType, WidgetIr, has_date_column, slash_header};

pub(super) fn fields(resource: &ResourceIr) -> String {
    let form_fields = resource
        .fields
        .iter()
        .filter(|field| field.usage.create || field.usage.update)
        .collect::<Vec<_>>();
    let mut output = slash_header(resource);
    render_presentation(resource, &form_fields, &mut output);
    render_query_fields(resource, &mut output);
    render_columns(resource, &mut output);
    render_form_fields(resource, &form_fields, &mut output);
    render_crud_resource(resource, &form_fields, &mut output);
    output
}

fn render_presentation(resource: &ResourceIr, form_fields: &[&FieldIr], output: &mut String) {
    let pascal = &resource.pascal_name;
    let date_parameter = if has_date_column(resource) {
        "  formatDate: (value: string) => string,\n"
    } else {
        ""
    };
    output.push_str("import {\n  defineFlatCrudResource,\n  type FlatCrudColumn,\n  type FlatCrudFormField,\n  type FlatCrudLabels,\n  type FlatCrudPermissions,\n  type FlatCrudQueryField,\n} from '@/components/business/flat-crud'\n");
    output.push_str(&format!(
        "import {{\n  create{pascal},\n  delete{pascal},\n  get{pascal},\n  list{pascal},\n  update{pascal},\n  type {pascal}CreateInput,\n  type {pascal}Query,\n  type {pascal}Record,\n  type {pascal}UpdateInput,\n}} from './api'\nimport {{ emptyPageResponse }} from '@/shared/http/types'\n\n"
    ));
    output.push_str(&format!("export interface {pascal}Form {{\n"));
    for field in form_fields {
        output.push_str(&format!("  {}: {}\n", field.name, field.typescript_type));
    }
    output.push_str("}\n\n");
    output.push_str(&format!(
        "export interface {pascal}Presentation {{\n  columns: readonly FlatCrudColumn<{pascal}Record>[]\n  formFields: readonly FlatCrudFormField<{pascal}Form>[]\n  labels: FlatCrudLabels\n  permissions: FlatCrudPermissions\n  queryFields: readonly FlatCrudQueryField<{pascal}Query>[]\n  resource: ReturnType<typeof {name}Resource>\n}}\n\ntype Translate = (zhCN: string, en: string) => string\n\nexport function create{pascal}Presentation(\n  translate: Translate,\n{date_parameter}): {pascal}Presentation {{\n",
        name = resource.name
    ));
    for field in resource
        .fields
        .iter()
        .filter(|field| !field.enum_values.is_empty())
    {
        output.push_str(&format!("  const {}Options = [\n", field.name));
        for (value, labels) in &field.enum_values {
            push_typescript_object(
                output,
                &[
                    format!("label: translate({:?}, {:?})", labels.zh_cn, labels.en),
                    format!("value: {}", ts_enum_value(value, field)),
                ],
            );
        }
        output.push_str("  ] as const\n\n");
    }
    render_labels(resource, output);
}

fn render_query_fields(resource: &ResourceIr, output: &mut String) {
    let pascal = &resource.pascal_name;
    output.push_str("  const queryFields = [\n");
    for field in resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && !matches!(field.widget, WidgetIr::Hidden))
    {
        push_typescript_object(output, &query_properties(field));
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudQueryField<{pascal}Query>[]\n\n"
    ));
}

fn render_columns(resource: &ResourceIr, output: &mut String) {
    let pascal = &resource.pascal_name;
    output.push_str("  const columns = [\n");
    for field in resource.fields.iter().filter(|field| field.usage.list) {
        push_typescript_object(output, &column_properties(field));
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudColumn<{pascal}Record>[]\n\n"
    ));
}

fn render_form_fields(resource: &ResourceIr, form_fields: &[&FieldIr], output: &mut String) {
    let pascal = &resource.pascal_name;
    output.push_str("  const formFields = [\n");
    for field in form_fields {
        push_typescript_object(output, &form_properties(field));
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudFormField<{pascal}Form>[]\n\n  return {{\n    columns,\n    formFields,\n    labels,\n    permissions,\n    queryFields,\n    resource: {name}Resource(translate),\n  }}\n}}\n\n",
        name = resource.name
    ));
}

pub(super) fn query_properties(field: &FieldIr) -> Vec<String> {
    let kind = if field.enum_values.is_empty() {
        "text"
    } else {
        "select"
    };
    let mut properties = vec![
        format!("key: {:?}", field.name),
        format!("kind: {kind:?}"),
        format!(
            "label: translate({:?}, {:?})",
            field.labels.zh_cn, field.labels.en
        ),
        format!(
            "placeholder: translate({:?}, {:?})",
            format!("请输入或选择{}", field.labels.zh_cn),
            format!("Enter or select {}", field.labels.en),
        ),
    ];
    if !field.enum_values.is_empty() {
        properties.push(format!("options: {}Options", field.name));
    }
    properties
}

pub(super) fn column_properties(field: &FieldIr) -> Vec<String> {
    let mut properties = vec![
        format!("key: {:?}", field.name),
        format!(
            "label: translate({:?}, {:?})",
            field.labels.zh_cn, field.labels.en
        ),
    ];
    if !field.enum_values.is_empty() {
        let positive = field.enum_values.keys().last().expect("枚举非空");
        properties.push("display: 'status'".into());
        properties.push(format!("options: {}Options", field.name));
        properties.push(format!("positiveValue: {}", ts_enum_value(positive, field)));
    } else if matches!(field.value_type, ValueType::DateTime) {
        properties.push("display: 'datetime'".into());
        properties.push("format: formatDate".into());
    }
    properties
}

pub(super) fn form_properties(field: &FieldIr) -> Vec<String> {
    let mut properties = vec![
        format!("key: {:?}", field.name),
        format!("kind: {:?}", form_widget(field)),
        format!(
            "label: translate({:?}, {:?})",
            field.labels.zh_cn, field.labels.en
        ),
    ];
    if matches!(field.widget, WidgetIr::Text | WidgetIr::Textarea) {
        properties.push(format!(
            "placeholder: translate({:?}, {:?})",
            format!("请输入{}", field.labels.zh_cn),
            format!("Enter {}", field.labels.en)
        ));
        if field.validation.required {
            properties.push(format!(
                "requiredMessage: translate({:?}, {:?})",
                format!("请输入{}", field.labels.zh_cn),
                format!("Enter {}", field.labels.en)
            ));
        }
    }
    if matches!(field.widget, WidgetIr::Number) {
        if let Some(minimum) = field.validation.minimum {
            properties.push(format!("min: {minimum}"));
        }
        if let Some(maximum) = field.validation.maximum {
            properties.push(format!("max: {maximum}"));
        }
    }
    if !field.enum_values.is_empty() {
        properties.push(format!("options: {}Options", field.name));
    }
    if field.usage.create && !field.usage.update {
        properties.push("disabledOnEdit: true".into());
    }
    if field.usage.update && !field.usage.create {
        properties.push("editOnly: true".into());
    }
    properties
}

pub(super) fn render_labels(resource: &ResourceIr, output: &mut String) {
    output.push_str(&format!(
        "  const permissions = {{\n    list: {:?},\n    create: {:?},\n    update: {:?},\n    remove: {:?},\n  }} as const\n\n",
        resource.access.permissions.list,
        resource.access.permissions.create,
        resource.access.permissions.update,
        resource.access.permissions.delete,
    ));
    output.push_str(&format!(
        "  const labels = {{\n    title: translate({:?}, {:?}),\n    add: translate(\"新增\", \"Add\"),\n    edit: translate(\"编辑\", \"Edit\"),\n    remove: translate(\"删除\", \"Delete\"),\n    actions: translate(\"操作\", \"Actions\"),\n    search: translate(\"搜索\", \"Search\"),\n    reset: translate(\"重置\", \"Reset\"),\n    confirm: translate(\"确定\", \"Confirm\"),\n    cancel: translate(\"取消\", \"Cancel\"),\n  }}\n\n",
        resource.labels.zh_cn, resource.labels.en
    ));
}
