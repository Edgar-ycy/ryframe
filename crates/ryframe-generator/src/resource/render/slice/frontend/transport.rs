use super::{ResourceIr, has_date_column, html_header, slash_header};

pub(in super::super) fn api(resource: &ResourceIr) -> String {
    let pascal = &resource.pascal_name;
    let domain = if resource.module == "business" {
        "system"
    } else {
        resource.module.as_str()
    };
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
        slash_header(resource),
        create_operation = operations.create,
        read_operation = operations.read,
        list_operation = operations.list,
        update_operation = operations.update,
        delete_operation = operations.delete,
    )
}

pub(in super::super) fn registration(resource: &ResourceIr) -> String {
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
      requiredCapabilities: {:?},
{system_tenant_only}      page: () => import({page:?}),
    }},
  ],
}})
"#,
        slash_header(resource),
        resource.route.key,
        resource.access.permissions.list,
        resource.route.path,
        resource
            .access
            .capability
            .as_ref()
            .map(|code| vec![code.as_str()])
            .unwrap_or_default(),
        system_tenant_only = if resource.access.system_tenant_only {
            "      requiresSystemTenant: true,\n"
        } else {
            ""
        },
    )
}

pub(in super::super) fn page(resource: &ResourceIr) -> String {
    let (date_import, date_argument) = if has_date_column(resource) {
        (
            "import { formatLocalizedDate } from '@/i18n'\n",
            ", formatLocalizedDate",
        )
    } else {
        ("", "")
    };
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
    <template v-if="slots.actions" #actions="{{ selectedExportIds, captureSelectionOwnership }}">
      <slot
        name="actions"
        :selected-export-ids="selectedExportIds"
        :capture-selection-ownership="captureSelectionOwnership"
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
{date_import}import type {{ {pascal}Query }} from './api'
import {{ create{pascal}Presentation }} from './fields'

const slots = defineSlots<{{
  actions?(props: {{ selectedExportIds: string[]; captureSelectionOwnership: () => () => boolean; canExport: boolean; lastSuccessfulQuery: {pascal}Query | null }}): unknown
}}>()

const {{ locale }} = useI18n()
const translate = (zhCN: string, en: string) => locale.value.startsWith('zh') ? zhCN : en
const presentation = computed(() => create{pascal}Presentation(translate{date_argument}))
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
