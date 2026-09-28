use super::{FieldIr, ResourceIr, ValueType, rust_base_type};

mod validation;

use validation::{
    render_enum_validators, render_utf8_byte_validators, schema_attribute, validation_attribute,
};

pub(super) fn dto(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let detail_import = if resource.relations.is_empty() {
        String::new()
    } else {
        format!(", {pascal}Detail")
    };
    let mut output = format!(
        "{header}use ryframe_application::generated::{name}::{{Create{pascal}Command, {pascal}ListParams, {pascal}Record{detail_import}, Update{pascal}Command}};\nuse ryframe_kernel::{{PaginationPolicy, ValidatedPageQuery}};\nuse serde::{{Deserialize, Serialize}};\nuse utoipa::{{IntoParams, ToSchema}};\nuse validator::Validate;\n\n",
        name = resource.name,
    );
    render_utf8_byte_validators(resource, &mut output);
    render_enum_validators(resource, &mut output);
    render_input(resource, &mut output, true);
    render_input(resource, &mut output, false);
    render_list_query(resource, &mut output);
    render_view(resource, &mut output);
    render_detail_view(resource, &mut output);
    output
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
        if let Some(attribute) = schema_attribute(field) {
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

fn render_detail_view(resource: &ResourceIr, output: &mut String) {
    if resource.relations.is_empty() {
        return;
    }
    let pascal = &resource.pascal_name;
    output.push_str("\n#[derive(Clone, Debug, Serialize, ToSchema)]\n");
    output.push_str(&format!("pub struct {pascal}DetailVo {{\n"));
    output.push_str("    #[serde(flatten)]\n");
    output.push_str(&format!("    pub record: {pascal}Vo,\n"));
    for relation in &resource.relations {
        output.push_str(&format!(
            "    pub {}: Option<crate::generated::{}::dto::{}Vo>,\n",
            relation.name, relation.target_resource, relation.target_pascal_name
        ));
    }
    output.push_str("}\n\n");
    output.push_str(&format!(
        "impl From<{pascal}Detail> for {pascal}DetailVo {{\n    fn from(value: {pascal}Detail) -> Self {{\n        Self {{\n            record: value.record.into(),\n"
    ));
    for relation in &resource.relations {
        output.push_str(&format!(
            "            {}: value.{}.map(Into::into),\n",
            relation.name, relation.name
        ));
    }
    output.push_str("        }\n    }\n}\n");
}

pub(super) fn handler(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let detail_vo = if resource.relations.is_empty() {
        format!("{pascal}Vo")
    } else {
        format!("{pascal}DetailVo")
    };
    let detail_import = if resource.relations.is_empty() {
        String::new()
    } else {
        format!(", {pascal}DetailVo")
    };
    let permissions = &resource.access.permissions;
    let operations = &resource.api.operations;
    let path = &resource.api.path;
    let tag = &resource.menu.labels.zh_cn;
    format!(
        include_str!("handler.rs.tpl"),
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
        header = header,
        pascal = pascal,
        detail_import = detail_import,
        path = path,
        tag = tag,
        detail_vo = detail_vo,
    )
}

pub(super) fn openapi(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let detail_schema = if resource.relations.is_empty() {
        String::new()
    } else {
        format!("        super::dto::{pascal}DetailVo,\n")
    };
    let (metadata, field_calls, field_functions) = metadata_parts(resource);
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
{detail_schema}
    ))
)]
pub struct {pascal}OpenApi;

/// OpenAPI 中 `x-ryframe-crud-resources` 使用的安全 UI 元数据。
pub fn crud_resource_metadata() -> serde_json::Value {{
    let mut metadata = serde_json::json!({metadata});
    metadata["fields"] = serde_json::json!([{field_calls}]);
    metadata
}}

{field_functions}
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

fn metadata_parts(resource: &ResourceIr) -> (String, String, String) {
    let mut metadata = super::super::catalog::crud_resource_metadata(resource);
    let fields = metadata
        .as_object_mut()
        .expect("资源元数据必须是对象")
        .remove("fields")
        .expect("资源元数据必须包含字段");
    let mut calls = Vec::new();
    let mut functions = Vec::new();
    for field in fields.as_array().expect("字段元数据必须是数组") {
        let name = field["name"].as_str().expect("字段名称已校验");
        let value = serde_json::to_string_pretty(field).expect("字段元数据可序列化");
        calls.push(format!("field_{name}()"));
        functions.push(format!(
            "fn field_{name}() -> serde_json::Value {{\n    serde_json::json!({value})\n}}\n"
        ));
    }
    (
        serde_json::to_string_pretty(&metadata).expect("资源元数据可序列化"),
        calls.join(", "),
        functions.join("\n"),
    )
}
