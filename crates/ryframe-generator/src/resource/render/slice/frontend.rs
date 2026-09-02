use super::super::super::{FieldIr, ResourceIr, ValueType, WidgetIr};
use super::super::{html_header, slash_header};

pub(super) fn api(resource: &ResourceIr) -> String {
    let pascal = &resource.pascal_name;
    let domain = &resource.module;
    let operations = &resource.api.operations;
    let detail_type = if resource.relations.is_empty() {
        String::new()
    } else {
        format!("export type {pascal}Detail = ApiSchema<'{pascal}DetailVo'>\n")
    };
    format!(
        r#"{}import {{
  {delete_operation},
  {list_operation},
  {read_operation},
  {create_operation},
  {update_operation},
}} from '@/api/generated/operations/{domain}'
import type {{ ApiSchema, OperationJsonBody, OperationQuery }} from '@/api/contract'
import type {{ Id }} from '@/shared/http/types'

export type {pascal}Record = ApiSchema<'{pascal}Vo'>
{detail_type}export type {pascal}Query = OperationQuery<'{list_operation}'>
export type {pascal}CreateInput = OperationJsonBody<'{create_operation}'>
export type {pascal}UpdateInput = OperationJsonBody<'{update_operation}'>

export function list{pascal}(params: {pascal}Query, signal?: AbortSignal) {{
  return {list_operation}({{ params, signal }})
}}

export function get{pascal}(id: Id, signal?: AbortSignal) {{
  return {read_operation}({{ path: {{ id }}, signal }})
}}

export function create{pascal}(data: {pascal}CreateInput) {{
  return {create_operation}({{ data }})
}}

export function update{pascal}(id: Id, data: {pascal}UpdateInput) {{
  return {update_operation}({{ path: {{ id }}, data }})
}}

export function delete{pascal}(id: Id) {{
  return {delete_operation}({{ path: {{ id }} }})
}}
"#,
        super::super::slash_header(resource),
        create_operation = operations.create,
        read_operation = operations.read,
        list_operation = operations.list,
        update_operation = operations.update,
        delete_operation = operations.delete,
    )
}

pub(super) fn fields(resource: &ResourceIr) -> String {
    render_fields(resource)
}

pub(super) fn page(resource: &ResourceIr) -> String {
    render_page(resource)
}

pub(super) fn registration(resource: &ResourceIr) -> String {
    let page = resource
        .frontend_extensions
        .get("page")
        .and_then(toml::Value::as_str)
        .unwrap_or("./page.vue");
    format!(
        r#"{}import {{ definePageManifest }} from '@/features/pages'

export const pageManifest = definePageManifest({{
  pages: [
    {{
      routeKey: {:?},
      permissionCode: {:?},
      path: {:?},
      page: () => import({page:?}),
    }},
  ],
}})
"#,
        super::super::slash_header(resource),
        resource.route.key,
        resource.access.permissions.list,
        resource.route.path,
    )
}

fn render_fields(resource: &ResourceIr) -> String {
    let pascal = &resource.pascal_name;
    let form_fields = resource
        .fields
        .iter()
        .filter(|field| field.usage.create || field.usage.update)
        .collect::<Vec<_>>();
    let mut output = slash_header(resource);
    output.push_str("import {\n  defineFlatCrudResource,\n  type FlatCrudColumn,\n  type FlatCrudFormField,\n  type FlatCrudLabels,\n  type FlatCrudPermissions,\n  type FlatCrudQueryField,\n} from '@/components/business/flat-crud'\n");
    output.push_str(&format!(
        "import {{\n  create{pascal},\n  delete{pascal},\n  get{pascal},\n  list{pascal},\n  update{pascal},\n  type {pascal}CreateInput,\n  type {pascal}Query,\n  type {pascal}Record,\n  type {pascal}UpdateInput,\n}} from './api'\nimport {{ emptyPageResponse }} from '@/shared/http/types'\n\n"
    ));
    output.push_str(&format!("export interface {pascal}Form {{\n"));
    for field in &form_fields {
        output.push_str(&format!("  {}: {}\n", field.name, field.typescript_type));
    }
    output.push_str("}\n\n");
    output.push_str(&format!(
        "export interface {pascal}Presentation {{\n  columns: readonly FlatCrudColumn<{pascal}Record>[]\n  formFields: readonly FlatCrudFormField<{pascal}Form>[]\n  labels: FlatCrudLabels\n  permissions: FlatCrudPermissions\n  queryFields: readonly FlatCrudQueryField<{pascal}Query>[]\n  resource: ReturnType<typeof {name}Resource>\n}}\n\ntype Translate = (zhCN: string, en: string) => string\n\nexport function create{pascal}Presentation(\n  translate: Translate,\n  formatDate: (value: string) => string,\n): {pascal}Presentation {{\n",
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
                &mut output,
                &[
                    format!("label: translate({:?}, {:?})", labels.zh_cn, labels.en),
                    format!("value: {}", ts_enum_value(value, field.value_type)),
                ],
            );
        }
        output.push_str("  ] as const\n\n");
    }
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
    output.push_str("  const queryFields = [\n");
    for field in resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && !matches!(field.widget, WidgetIr::Hidden))
    {
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
        push_typescript_object(&mut output, &properties);
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudQueryField<{pascal}Query>[]\n\n"
    ));
    output.push_str("  const columns = [\n");
    for field in resource.fields.iter().filter(|field| field.usage.list) {
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
            properties.push(format!(
                "positiveValue: {}",
                ts_enum_value(positive, field.value_type)
            ));
        } else if matches!(field.value_type, ValueType::DateTime) {
            properties.push("display: 'datetime'".into());
            properties.push("format: formatDate".into());
        }
        push_typescript_object(&mut output, &properties);
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudColumn<{pascal}Record>[]\n\n"
    ));
    output.push_str("  const formFields = [\n");
    for field in &form_fields {
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
        push_typescript_object(&mut output, &properties);
    }
    output.push_str(&format!(
        "  ] as const satisfies readonly FlatCrudFormField<{pascal}Form>[]\n\n  return {{\n    columns,\n    formFields,\n    labels,\n    permissions,\n    queryFields,\n    resource: {name}Resource(translate),\n  }}\n}}\n\n",
        name = resource.name
    ));

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
        .map(|field| {
            let value = if field.nullable {
                format!("record.{} ?? null", field.name)
            } else {
                format!("record.{}", field.name)
            };
            format!("      {}: {value},\n", field.name)
        })
        .collect::<String>();
    let create_input = resource
        .fields
        .iter()
        .filter(|field| field.usage.create)
        .map(|field| format!("      {}: form.{},\n", field.name, field.name))
        .collect::<String>();
    let update_input = resource
        .fields
        .iter()
        .filter(|field| field.usage.update)
        .map(|field| format!("      {}: form.{},\n", field.name, field.name))
        .collect::<String>();
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
    output.push_str(&format!(
        r#"function {name}Resource(translate: Translate) {{
  return defineFlatCrudResource<
    {pascal}Record,
    {pascal}Query,
    {pascal}Form,
    {pascal}CreateInput,
    {pascal}UpdateInput
  >({{
    key: {name:?},
    initialQuery: (): {pascal}Query => ({{
      page: 1,
      page_size: 10,
{query_defaults}    }}),
    emptyForm: (): {pascal}Form => ({{
{empty_form}    }}),
    editForm: record => ({{
{edit_form}    }}),
    createInput: form => ({{
{create_input}    }}),
    updateInput: form => ({{
{update_input}    }}),
    recordId: record => String(record.{record_id}),
    messages: {{
      addSuccess: translate("新增成功", "Created"),
      addTitle: translate("新增{zh}", "Add {en}"),
      deleteConfirm: record => translate(
        `确定删除 ${{String(record.{record_label})}} 吗？`,
        `Delete ${{String(record.{record_label})}}?`,
      ),
      deleteSuccess: translate("删除成功", "Deleted"),
      detailMissing: translate("资源不存在", "Resource not found"),
      editTitle: translate("编辑{zh}", "Edit {en}"),
      updateSuccess: translate("更新成功", "Updated"),
      warningTitle: translate("提示", "Warning"),
    }},
    adapter: {{
      async list(query, signal) {{
        const response = await list{pascal}({{ ...query }}, signal)
        return response.data ?? emptyPageResponse<{pascal}Record>(query)
      }},
      async detail(id, signal) {{
        const response = await get{pascal}(id, signal)
        if (!response.data) throw new Error(translate("资源不存在", "Resource not found"))
        return response.data
      }},
      async create(input) {{ await create{pascal}(input) }},
      async update(id, input) {{ await update{pascal}(id, input) }},
      async remove(id) {{ await delete{pascal}(id) }},
    }},
  }})
}}
"#,
        name = resource.name,
        zh = resource.labels.zh_cn,
        en = resource.labels.en,
    ));
    output
}

fn render_page(resource: &ResourceIr) -> String {
    format!(
        r#"{}<template>
  <FlatCrudPage
    v-model:dialog-visible="dialogVisible"
    v-model:form="form"
    v-model:page="page"
    v-model:page-size="pageSize"
    :columns="presentation.columns"
    :deleting-key="deletingKey"
    :dialog-title="dialogTitle"
    :editing="editing"
    :form-fields="presentation.formFields"
    :labels="presentation.labels"
    :loading="listQuery.isFetching.value"
    :permissions="presentation.permissions"
    :query="query"
    :query-fields="presentation.queryFields"
    :row-key="presentation.resource.recordId"
    :rows="listQuery.data.value?.items ?? []"
    :saving="saving"
    :total="listQuery.data.value?.total ?? 0"
    @add="add"
    @edit="edit"
    @page-change="changePage"
    @remove="remove"
    @reset="reset"
    @search="search"
    @submit="submit"
    @update:query="setQuery"
  >
    <template v-if="slots.actions" #actions>
      <slot
        name="actions"
        :can-export="canExport"
        :last-successful-query="lastSuccessfulQuery ?? null"
      />
    </template>
  </FlatCrudPage>
</template>

<script setup lang="ts">
import {{ computed }} from 'vue'
import {{ useI18n }} from 'vue-i18n'

import {{ FlatCrudPage, useFlatCrudResource }} from '@/components/business/flat-crud'
import {{ formatLocalizedDate }} from '@/i18n'
import type {{ {pascal}Query }} from './api'
import {{ create{pascal}Presentation }} from './fields'

const slots = defineSlots<{{
  actions?(props: {{ canExport: boolean; lastSuccessfulQuery: {pascal}Query | null }}): unknown
}}>()

const {{ locale }} = useI18n()
const translate = (zhCN: string, en: string) => locale.value.startsWith('zh') ? zhCN : en
const presentation = computed(() => create{pascal}Presentation(translate, formatLocalizedDate))
const {{
  add,
  canExport,
  changePage,
  deletingKey,
  dialogTitle,
  dialogVisible,
  edit,
  editing,
  form,
  lastSuccessfulQuery,
  listQuery,
  page,
  pageSize,
  query,
  remove,
  reset,
  search,
  setQuery,
  submit,
  saving,
}} = useFlatCrudResource(presentation.value.resource)
</script>
"#,
        html_header(resource),
        pascal = resource.pascal_name,
    )
}

fn push_typescript_object(output: &mut String, properties: &[String]) {
    output.push_str("    {\n");
    for property in properties {
        output.push_str("      ");
        output.push_str(property);
        output.push_str(",\n");
    }
    output.push_str("    },\n");
}

fn ts_default(field: &FieldIr) -> &'static str {
    match field.value_type {
        ValueType::String | ValueType::Date | ValueType::DateTime => "''",
        ValueType::I32 | ValueType::I64 | ValueType::Decimal => "0",
        ValueType::Bool => "false",
        ValueType::Json => "null",
    }
}

fn ts_enum_value(value: &str, value_type: ValueType) -> String {
    match value_type {
        ValueType::I32 | ValueType::I64 | ValueType::Decimal if value.parse::<f64>().is_ok() => {
            value.to_owned()
        }
        ValueType::Bool if matches!(value, "true" | "false") => value.to_owned(),
        _ => format!("{value:?}"),
    }
}

fn form_widget(field: &FieldIr) -> &'static str {
    if !field.enum_values.is_empty() {
        "radio"
    } else {
        match field.widget {
            WidgetIr::Number => "number",
            _ => "text",
        }
    }
}
