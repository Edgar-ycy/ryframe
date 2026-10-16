use super::modules::Modules;
use super::presentation::render_labels;
use super::resource::{Mappings, definition};
use super::{ResourceIr, has_date_column};

pub(super) fn render(resource: &ResourceIr) -> Vec<(String, String)> {
    let mut modules = Modules {
        resource,
        assets: Vec::new(),
    };
    modules.add(
        "common.ts",
        "export type Translate = (zhCN: string, en: string) => string\n",
    );
    super::split_form::render(&mut modules);
    super::split_presentation::render(&mut modules);
    modules.add("resource.ts", resource_module(resource));
    modules.add("fields.ts", presentation_module(resource));
    modules.assets
}

fn resource_module(resource: &ResourceIr) -> String {
    let pascal = &resource.pascal_name;
    let mut output = format!(
        "import {{ defineFlatCrudResource }} from '@/components/business/flat-crud'\nimport {{ emptyPageResponse }} from '@/shared/http/types'\nimport {{\n  create{pascal},\n  delete{pascal},\n  get{pascal},\n  list{pascal},\n  update{pascal},\n  type {pascal}CreateInput,\n  type {pascal}UpdateInput,\n  type {pascal}Query,\n  type {pascal}Record,\n}} from './api'\nimport type {{ {pascal}Form }} from './model'\nimport type {{ Translate }} from './common'\nimport {{ emptyForm, editForm, createInput, updateInput }} from './form'\nimport {{ queryDefaults }} from './query-defaults'\n\n"
    );
    let mappings = Mappings {
        query_defaults: "      ...queryDefaults(),\n".into(),
        empty_form: "      ...emptyForm(),\n".into(),
        edit_form: "      ...editForm(record),\n".into(),
        create_input: "      ...createInput(form),\n".into(),
        update_input: "      ...updateInput(form),\n".into(),
    };
    output.push_str(&definition(resource, &mappings).replacen("function ", "export function ", 1));
    output
}

fn presentation_module(resource: &ResourceIr) -> String {
    let pascal = &resource.pascal_name;
    let name = &resource.name;
    let mut output = format!(
        "import type {{\n  FlatCrudColumn,\n  FlatCrudFormField,\n  FlatCrudLabels,\n  FlatCrudPermissions,\n  FlatCrudQueryField,\n}} from '@/components/business/flat-crud'\nimport type {{ {pascal}Query, {pascal}Record }} from './api'\nimport type {{ {pascal}Form }} from './model'\nimport type {{ Translate }} from './common'\nimport {{ columns }} from './columns'\nimport {{ formFields }} from './form-fields'\nimport {{ queryFields }} from './query'\nimport {{ {name}Resource }} from './resource'\n\nexport type {{ {pascal}Form }} from './model'\n\nexport interface {pascal}Presentation {{\n  columns: readonly FlatCrudColumn<{pascal}Record>[]\n  formFields: readonly FlatCrudFormField<{pascal}Form>[]\n  labels: FlatCrudLabels\n  permissions: FlatCrudPermissions\n  queryFields: readonly FlatCrudQueryField<{pascal}Query>[]\n  resource: ReturnType<typeof {name}Resource>\n}}\n\n"
    );
    let (date_parameter, date_argument) = if has_date_column(resource) {
        ("  formatDate: (value: string) => string,\n", ", formatDate")
    } else {
        ("", "")
    };
    output.push_str(&format!("export function create{pascal}Presentation(\n  translate: Translate,\n{date_parameter}): {pascal}Presentation {{\n"));
    render_labels(resource, &mut output);
    output.push_str(&format!("  return {{\n    columns: columns(translate{date_argument}),\n    formFields: formFields(translate),\n    labels,\n    permissions,\n    queryFields: queryFields(translate),\n    resource: {name}Resource(translate),\n  }}\n}}\n"));
    output
}
