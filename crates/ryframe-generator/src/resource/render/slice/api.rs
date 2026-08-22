use super::{FieldIr, ResourceIr, ValueType, rust_base_type};

pub(super) fn dto(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let mut output = format!(
        "{header}use ryframe_application::generated::{name}::{{Create{pascal}Command, {pascal}ListParams, {pascal}Record, Update{pascal}Command}};\nuse ryframe_kernel::{{PaginationPolicy, ValidatedPageQuery}};\nuse serde::{{Deserialize, Serialize}};\nuse utoipa::{{IntoParams, ToSchema}};\nuse validator::Validate;\n\n",
        name = resource.name,
    );
    render_enum_validators(resource, &mut output);
    render_input(resource, &mut output, true);
    render_input(resource, &mut output, false);
    render_list_query(resource, &mut output);
    render_view(resource, &mut output);
    output
}

fn render_enum_validators(resource: &ResourceIr, output: &mut String) {
    for field in resource
        .fields
        .iter()
        .filter(|field| (field.usage.create || field.usage.update) && !field.enum_values.is_empty())
    {
        let (argument_type, values) = match field.value_type {
            ValueType::String => (
                "&str",
                field
                    .enum_values
                    .keys()
                    .map(|value| format!("{value:?}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ValueType::I32 => (
                "i32",
                field
                    .enum_values
                    .keys()
                    .map(|value| format!("{value}_i32"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ValueType::I64 => (
                "i64",
                field
                    .enum_values
                    .keys()
                    .map(|value| format!("{value}_i64"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            ValueType::Bool => (
                "bool",
                field
                    .enum_values
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            _ => unreachable!("枚举 value_type 已在 IR 边界校验"),
        };
        output.push_str(&format!(
            "fn validate_{name}_enum(value: {argument_type}) -> Result<(), validator::ValidationError> {{\n    if [{values}].contains(&value) {{\n        Ok(())\n    }} else {{\n        Err(validator::ValidationError::new({error:?}))\n    }}\n}}\n\n",
            name = field.name,
            error = format!("invalid_{}", field.name),
        ));
    }
}

fn render_input(resource: &ResourceIr, output: &mut String, create: bool) {
    let pascal = &resource.pascal_name;
    let prefix = if create { "Create" } else { "Update" };
    output.push_str("#[derive(Debug, Deserialize, Validate, ToSchema)]\n");
    output.push_str("#[serde(deny_unknown_fields)]\n");
    output.push_str(&format!("pub struct {prefix}{pascal}Dto {{\n"));
    for field in resource.fields.iter().filter(|field| {
        if create {
            field.usage.create
        } else {
            field.usage.update
        }
    }) {
        if let Some(attribute) = validation_attribute(field) {
            output.push_str(&format!("    {attribute}\n"));
        }
        let optional = if create {
            field.usage.create_optional
        } else {
            field.usage.update_optional
        } || field.nullable;
        let value_type = rust_base_type(field.value_type);
        let value_type = if optional {
            format!("Option<{value_type}>")
        } else {
            value_type.into()
        };
        output.push_str(&format!("    pub {}: {value_type},\n", field.name));
    }
    output.push_str("}\n\n");

    output.push_str(&format!(
        "impl From<{prefix}{pascal}Dto> for {prefix}{pascal}Command {{\n    fn from(value: {prefix}{pascal}Dto) -> Self {{\n        Self {{\n"
    ));
    for field in resource.fields.iter().filter(|field| {
        if create {
            field.usage.create
        } else {
            field.usage.update
        }
    }) {
        output.push_str(&format!(
            "            {}: value.{},\n",
            field.name, field.name
        ));
    }
    output.push_str("        }\n    }\n}\n\n");
}

fn render_list_query(resource: &ResourceIr, output: &mut String) {
    let pascal = &resource.pascal_name;
    output.push_str("#[derive(Debug, Deserialize, IntoParams, ToSchema)]\n");
    output.push_str("#[serde(deny_unknown_fields)]\n#[into_params(parameter_in = Query)]\n");
    output.push_str(&format!("pub struct {pascal}ListQuery {{\n"));
    output.push_str("    #[param(minimum = 1)]\n    pub page: Option<u64>,\n");
    output.push_str("    #[param(minimum = 1)]\n    pub page_size: Option<u64>,\n");
    for field in filter_fields(resource) {
        output.push_str(&format!(
            "    pub {}: Option<{}>,\n",
            field.name,
            wire_base_type(field.wire_type)
        ));
    }
    output.push_str("}\n\n");
    output.push_str(&format!(
        "impl {pascal}ListQuery {{\n    pub fn into_service_params(self, policy: PaginationPolicy) -> crate::http::HttpResult<{pascal}ListParams> {{\n        Ok({pascal}ListParams {{\n            page: ValidatedPageQuery::from_optional(self.page, self.page_size, policy)?,\n"
    ));
    for field in filter_fields(resource) {
        output.push_str(&format!(
            "            {}: {},\n",
            field.name,
            filter_conversion(field)
        ));
    }
    output.push_str("        })\n    }\n}\n\n");
}

fn render_view(resource: &ResourceIr, output: &mut String) {
    let pascal = &resource.pascal_name;
    output.push_str("#[derive(Clone, Debug, Serialize, ToSchema)]\n");
    output.push_str(&format!("pub struct {pascal}Vo {{\n"));
    for field in view_fields(resource) {
        output.push_str(&format!("    pub {}: {},\n", field.name, wire_type(field)));
    }
    output.push_str("}\n\n");
    output.push_str(&format!(
        "impl From<{pascal}Record> for {pascal}Vo {{\n    fn from(value: {pascal}Record) -> Self {{\n        Self {{\n"
    ));
    for field in view_fields(resource) {
        output.push_str(&format!(
            "            {}: {},\n",
            field.name,
            view_conversion(field)
        ));
    }
    output.push_str("        }\n    }\n}\n");
}

pub(super) fn handler(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let permissions = &resource.access.permissions;
    let operations = &resource.api.operations;
    let path = &resource.api.path;
    let tag = &resource.menu.labels.zh_cn;
    format!(
        r#"{header}use std::sync::Arc;

use axum::{{
    Json, Router,
    extract::{{Path, Query, State}},
}};
use ryframe_application::generated::{name}::{pascal}Service;
use ryframe_kernel::{{AppError, PaginationPolicy}};
use ryframe_macro::{{delete, get, post, put, route}};
use validator::Validate;

use crate::RequestPrincipal;
use crate::http::{{ApiPageResponse, ApiResponse, HttpAppError, HttpResult}};

use super::dto::{{Create{pascal}Dto, {pascal}ListQuery, {pascal}Vo, Update{pascal}Dto}};

#[derive(Clone)]
pub struct {pascal}HttpState {{
    service: Arc<{pascal}Service>,
    pagination: PaginationPolicy,
}}

pub fn router(service: Arc<{pascal}Service>, pagination: PaginationPolicy) -> Router {{
    Router::new()
        .merge(route!(list))
        .merge(route!(detail))
        .merge(route!(create))
        .merge(route!(update))
        .merge(route!(remove))
        .with_state({pascal}HttpState {{ service, pagination }})
}}

#[get("/")]
#[perm({list_permission:?})]
#[utoipa::path(
    get,
    path = {path:?},
    operation_id = {list_operation:?},
    tag = {tag:?},
    params({pascal}ListQuery),
    responses((status = 200, description = "列表", body = ApiPageResponse<{pascal}Vo>)),
    security(("bearer" = []))
)]
pub async fn list(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Query(query): Query<{pascal}ListQuery>,
) -> HttpResult<Json<ApiPageResponse<{pascal}Vo>>> {{
    let page = state
        .service
        .find_by_page(&current_user, query.into_service_params(state.pagination)?)
        .await
        .map_err(HttpAppError::from)?;
    Ok(Json(ApiPageResponse::page(
        page.records.into_iter().map({pascal}Vo::from).collect(),
        page.total,
        page.page,
        page.page_size,
        state.pagination.max_page_size(),
    )))
}}

#[get("/{{id}}")]
#[perm({read_permission:?})]
#[utoipa::path(
    get,
    path = {detail_path:?},
    operation_id = {read_operation:?},
    tag = {tag:?},
    params(("id" = String, Path)),
    responses((status = 200, description = "详情", body = ApiResponse<{pascal}Vo>)),
    security(("bearer" = []))
)]
pub async fn detail(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<{pascal}Vo>>> {{
    let value = state
        .service
        .find_by_id(&current_user, parse_id(&id)?)
        .await
        .map_err(HttpAppError::from)?
        .ok_or_else(|| HttpAppError::from(AppError::NotFound({not_found:?}.into())))?;
    Ok(Json(ApiResponse::success(value.into())))
}}

#[post("/")]
#[perm({create_permission:?})]
#[utoipa::path(
    post,
    path = {path:?},
    operation_id = {create_operation:?},
    tag = {tag:?},
    request_body = Create{pascal}Dto,
    responses((status = 200, description = "创建成功", body = ApiResponse<{pascal}Vo>)),
    security(("bearer" = []))
)]
pub async fn create(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Json(dto): Json<Create{pascal}Dto>,
) -> HttpResult<Json<ApiResponse<{pascal}Vo>>> {{
    dto.validate()?;
    let value = state
        .service
        .create(&current_user, dto.into())
        .await
        .map_err(HttpAppError::from)?;
    Ok(Json(ApiResponse::success(value.into())))
}}

#[put("/{{id}}")]
#[perm({update_permission:?})]
#[utoipa::path(
    put,
    path = {detail_path:?},
    operation_id = {update_operation:?},
    tag = {tag:?},
    params(("id" = String, Path)),
    request_body = Update{pascal}Dto,
    responses((status = 200, description = "更新成功", body = ApiResponse<{pascal}Vo>)),
    security(("bearer" = []))
)]
pub async fn update(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
    Json(dto): Json<Update{pascal}Dto>,
) -> HttpResult<Json<ApiResponse<{pascal}Vo>>> {{
    dto.validate()?;
    let value = state
        .service
        .update(&current_user, parse_id(&id)?, dto.into())
        .await
        .map_err(HttpAppError::from)?;
    Ok(Json(ApiResponse::success(value.into())))
}}

#[delete("/{{id}}")]
#[perm({delete_permission:?})]
#[utoipa::path(
    delete,
    path = {detail_path:?},
    operation_id = {delete_operation:?},
    tag = {tag:?},
    params(("id" = String, Path)),
    responses((status = 200, description = "删除成功", body = crate::http::ApiEmptyResponse)),
    security(("bearer" = []))
)]
pub async fn remove(
    State(state): State<{pascal}HttpState>,
    current_user: RequestPrincipal,
    Path(id): Path<String>,
) -> HttpResult<Json<ApiResponse<()>>> {{
    state
        .service
        .delete(&current_user, parse_id(&id)?)
        .await
        .map_err(HttpAppError::from)?;
    Ok(Json(ApiResponse::success_no_data()))
}}

fn parse_id(value: &str) -> HttpResult<i64> {{
    value
        .parse()
        .map_err(|_| HttpAppError::from(AppError::Validation("id 必须是 i64 字符串".into())))
}}
"#,
        name = resource.name,
        list_permission = permissions.list,
        read_permission = permissions.read,
        create_permission = permissions.create,
        update_permission = permissions.update,
        delete_permission = permissions.delete,
        list_operation = operations.list,
        read_operation = operations.read,
        create_operation = operations.create,
        update_operation = operations.update,
        delete_operation = operations.delete,
        detail_path = format!("{path}/{{id}}"),
        not_found = format!("{}不存在", resource.labels.zh_cn),
    )
}

pub(super) fn openapi(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let metadata = super::super::catalog::crud_resource_metadata(resource);
    let metadata =
        serde_json::to_string_pretty(&metadata).expect("资源元数据只包含可序列化的稳定值");
    format!(
        r#"{header}use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        super::handler::list,
        super::handler::detail,
        super::handler::create,
        super::handler::update,
        super::handler::remove,
    ),
    components(schemas(
        super::dto::Create{pascal}Dto,
        super::dto::Update{pascal}Dto,
        super::dto::{pascal}ListQuery,
        super::dto::{pascal}Vo,
    ))
)]
pub struct {pascal}OpenApi;

/// OpenAPI 中 `x-ryframe-crud-resources` 使用的安全 UI 元数据。
pub fn crud_resource_metadata() -> serde_json::Value {{
    serde_json::json!({metadata})
}}
"#
    )
}

fn filter_fields(resource: &ResourceIr) -> impl Iterator<Item = &FieldIr> {
    resource
        .fields
        .iter()
        .filter(|field| field.usage.filter && field.name != "tenant_id")
}

fn view_fields(resource: &ResourceIr) -> impl Iterator<Item = &FieldIr> {
    resource
        .fields
        .iter()
        .filter(|field| field.usage.read || field.usage.list)
}

fn validation_attribute(field: &FieldIr) -> Option<String> {
    let validation = &field.validation;
    let mut validators = Vec::new();
    match field.value_type {
        ValueType::String
            if validation.required
                || validation.min_length.is_some()
                || validation.max_length.is_some() =>
        {
            let mut parts = Vec::new();
            let minimum = validation
                .min_length
                .map(|minimum| minimum.max(u32::from(validation.required)))
                .or_else(|| validation.required.then_some(1));
            if let Some(minimum) = minimum {
                parts.push(format!("min = {minimum}"));
            }
            if let Some(maximum) = validation.max_length {
                parts.push(format!("max = {maximum}"));
            }
            validators.push(format!("length({})", parts.join(", ")));
        }
        ValueType::I32 | ValueType::I64
            if validation.minimum.is_some() || validation.maximum.is_some() =>
        {
            let mut parts = Vec::new();
            if let Some(minimum) = validation.minimum {
                parts.push(format!("min = {minimum}"));
            }
            if let Some(maximum) = validation.maximum {
                parts.push(format!("max = {maximum}"));
            }
            validators.push(format!("range({})", parts.join(", ")));
        }
        _ => {}
    }
    if !field.enum_values.is_empty() {
        validators.push(format!(
            "custom(function = \"validate_{}_enum\")",
            field.name
        ));
    }
    if validators.is_empty() {
        None
    } else {
        Some(format!("#[validate({})]", validators.join(", ")))
    }
}

fn wire_base_type(value_type: ValueType) -> &'static str {
    match value_type {
        ValueType::String => "String",
        ValueType::I32 => "i32",
        ValueType::I64 => "i64",
        ValueType::Bool => "bool",
        ValueType::Date => "chrono::NaiveDate",
        ValueType::DateTime => "chrono::DateTime<chrono::Utc>",
        ValueType::Json => "serde_json::Value",
        ValueType::Decimal => unreachable!("decimal 已在生成边界校验中拒绝"),
    }
}

fn wire_type(field: &FieldIr) -> String {
    let value_type = wire_base_type(field.wire_type);
    if field.nullable {
        format!("Option<{value_type}>")
    } else {
        value_type.into()
    }
}

fn filter_conversion(field: &FieldIr) -> String {
    if field.value_type == ValueType::I64 && field.wire_type == ValueType::String {
        format!(
            "self.{0}.map(|value| value.parse::<i64>().map_err(|_| ryframe_kernel::AppError::Validation(\"{0} 必须是 i64 字符串\".into()))).transpose()?",
            field.name
        )
    } else {
        format!("self.{}", field.name)
    }
}

fn view_conversion(field: &FieldIr) -> String {
    if field.value_type == ValueType::I64 && field.wire_type == ValueType::String {
        if field.nullable {
            format!("value.{}.map(|item| item.to_string())", field.name)
        } else {
            format!("value.{}.to_string()", field.name)
        }
    } else {
        format!("value.{}", field.name)
    }
}
