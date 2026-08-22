use super::ResourceIr;

pub(super) fn api(resource: &ResourceIr) -> String {
    let pascal = &resource.pascal_name;
    let operations = &resource.api.operations;
    format!(
        r#"{}import {{ requestOperation }} from '@/api/operationRequest'
import {{
  {delete_operation},
  {list_operation},
  {read_operation},
  {create_operation},
  {update_operation},
}} from '@/api/generated/operations'
import type {{ ApiSchema, OperationJsonBody, OperationQuery }} from '@/api/contract'
import type {{ Id }} from '@/shared/http/types'

export type {pascal}Record = ApiSchema<'{pascal}Vo'>
export type {pascal}Query = OperationQuery<'{list_operation}'>
export type {pascal}CreateInput = OperationJsonBody<'{create_operation}'>
export type {pascal}UpdateInput = OperationJsonBody<'{update_operation}'>

export function list{pascal}(params: {pascal}Query, signal?: AbortSignal) {{
  return requestOperation({list_operation}, {{ params, signal }})
}}

export function get{pascal}(id: Id, signal?: AbortSignal) {{
  return requestOperation({read_operation}, {{ path: {{ id }}, signal }})
}}

export function create{pascal}(data: {pascal}CreateInput) {{
  return requestOperation({create_operation}, {{ data }})
}}

export function update{pascal}(id: Id, data: {pascal}UpdateInput) {{
  return requestOperation({update_operation}, {{ path: {{ id }}, data }})
}}

export function delete{pascal}(id: Id) {{
  return requestOperation({delete_operation}, {{ path: {{ id }} }})
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
    super::super::render_frontend_fields(resource)
}

pub(super) fn page(resource: &ResourceIr) -> String {
    super::super::render_frontend_page(resource)
}
