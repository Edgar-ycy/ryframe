use super::{FieldIr, ResourceIr, ValueType, rust_base_type};

mod validation;

use validation::{
    render_decimal_validators, render_enum_validators, render_utf8_byte_validators,
    schema_attribute, validation_attribute,
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
    render_decimal_validators(resource, &mut output);
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
    let versioned_update = !create && resource.revision_field.is_some();
    output.push_str("#[derive(Debug, Deserialize, Validate, ToSchema)]\n");
    output.push_str("#[serde(deny_unknown_fields)]\n");
    output.push_str(&format!("pub struct {prefix}{pascal}Dto {{\n"));
    if versioned_update {
        output.push_str(concat!(
            "    #[schema(value_type = String, pattern = r\"^(?:0|[1-9][0-9]*)$\")]\n",
            "    #[serde(deserialize_with = \"crate::http::integer::deserialize_u64\")]\n",
            "    pub expected_revision: u64,\n",
        ));
    }
    for field in resource.fields.iter().filter(|field| {
        if create {
            field.usage.create
        } else {
            field.usage.update
        }
    }) {
        let optional = (if create {
            field.usage.create_optional
        } else {
            field.usage.update_optional
        }) || field.nullable;
        if let Some(attribute) = validation_attribute(field) {
            output.push_str(&format!("    {attribute}\n"));
        }
        if let Some(attribute) = schema_attribute(field, optional) {
            output.push_str(&format!("    {attribute}\n"));
        }
        if matches!(field.value_type, ValueType::I64 | ValueType::U64)
            && field.wire_type == ValueType::String
        {
            let function = format!(
                "deserialize_{}{}",
                if optional { "optional_" } else { "" },
                field.value_type.rust_base_type()
            );
            let default = if optional { "default, " } else { "" };
            output.push_str(&format!(
                "    #[serde({default}deserialize_with = \"crate::http::integer::{function}\")]\n"
            ));
        }
        if field.value_type == ValueType::Decimal {
            let function = if optional {
                "deserialize_optional_decimal"
            } else {
                "deserialize_decimal"
            };
            let default = if optional { "default, " } else { "" };
            output.push_str(&format!(
                "    #[serde({default}deserialize_with = \"crate::http::decimal::{function}\")]\n"
            ));
        }
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
    if versioned_update {
        output.push_str("            expected_revision: value.expected_revision,\n");
    }
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
    let capability_attribute = resource
        .access
        .capability
        .as_deref()
        .map(|code| format!("#[ryframe_macro::capability({code:?})]\n"))
        .unwrap_or_default();
    format!(
        include_str!("handler.rs.tpl"),
        id_type = super::id_type(resource),
        id_parser = if super::id_type(resource) == "u64" {
            "parse_unsigned_id"
        } else {
            "parse_id"
        },
        id_parser_path = if super::id_type(resource) == "u64" {
            "crate"
        } else {
            "crate::handler_utils"
        },
        name = resource.name,
        list_permission = permissions.list,
        read_permission = permissions.read,
        create_permission = permissions.create,
        update_permission = permissions.update,
        delete_permission = permissions.delete,
        capability_attribute = capability_attribute,
        list_operation = operations.list,
        read_operation = operations.read,
        create_operation = operations.create,
        update_operation = operations.update,
        delete_operation = operations.delete,
        detail_path = format!("{path}/{{id}}"),
        revision_conflict = if resource.revision_field.is_some() {
            ", (status = 409, description = \"记录版本冲突\")"
        } else {
            ""
        },
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
    let (metadata, field_calls, field_functions) = metadata_parts(resource);
    openapi_document(resource, header, &metadata, &field_calls, &field_functions)
}

pub(super) fn business_openapi(
    resource: &ResourceIr,
    header: &str,
) -> (String, Vec<(String, String)>) {
    let (metadata, fields) = metadata_fields(resource);
    let mut assets = Vec::new();
    let mut modules = String::new();
    let mut calls = Vec::new();
    for field in fields {
        let name = field["name"].as_str().expect("字段名称已校验");
        let function = format!("field_{name}");
        modules.push_str(&format!(
            "mod {function};\npub(super) use {function}::{function};\n"
        ));
        calls.push(format!("fields::{function}()"));
        let value = super::super::catalog::rust_json(&field);
        assets.push((format!("{function}.rs"), format!("{header}pub(crate) fn {function}() -> serde_json::Value {{\n    serde_json::json!({value})\n}}\n")));
    }
    assets.push(("mod.rs".into(), format!("{header}{modules}")));
    let declaration = format!(
        "#[path = {:?}]\nmod fields;",
        format!("{}/fields/mod.rs", resource.name)
    );
    (
        openapi_document(resource, header, &metadata, &calls.join(", "), &declaration),
        assets,
    )
}

fn openapi_document(
    resource: &ResourceIr,
    header: &str,
    metadata: &str,
    field_calls: &str,
    field_functions: &str,
) -> String {
    let pascal = &resource.pascal_name;
    let detail_schema = if resource.relations.is_empty() {
        String::new()
    } else {
        format!("        super::dto::{pascal}DetailVo,\n")
    };
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
    value_type.rust_base_type()
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
    if field.value_type == ValueType::Decimal {
        let (precision, scale) = crate::resource::decimal::capacity(
            field.sql_type.as_deref().expect("Decimal 类型已校验"),
        )
        .expect("Decimal 容量已校验");
        return format!(
            "self.{0}.map(|value| crate::http::decimal::parse_decimal_with_capacity(&value, {precision}, {scale}).map_err(|reason| ryframe_kernel::AppError::Validation(format!(\"{0}: {{reason}}\")))).transpose()?",
            field.name
        );
    }
    if matches!(
        field.value_type,
        ValueType::I64 | ValueType::U64 | ValueType::Decimal
    ) && field.wire_type == ValueType::String
    {
        let kind = field.value_type.rust_base_type();
        format!(
            "self.{0}.map(|value| crate::http::integer::parse_{kind}(&value).map_err(|_| ryframe_kernel::AppError::Validation(\"{0} 必须是 {kind} 范围内的规范十进制字符串\".into()))).transpose()?",
            field.name
        )
    } else {
        format!("self.{}", field.name)
    }
}

fn view_conversion(field: &FieldIr) -> String {
    if matches!(
        field.value_type,
        ValueType::I64 | ValueType::U64 | ValueType::Decimal
    ) && field.wire_type == ValueType::String
    {
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
    let (metadata, fields) = metadata_fields(resource);
    let mut calls = Vec::new();
    let mut functions = Vec::new();
    for field in fields {
        let name = field["name"].as_str().expect("字段名称已校验");
        let value = super::super::catalog::rust_json(&field);
        calls.push(format!("field_{name}()"));
        functions.push(format!(
            "fn field_{name}() -> serde_json::Value {{\n    serde_json::json!({value})\n}}\n"
        ));
    }
    (metadata, calls.join(", "), functions.join("\n"))
}

fn metadata_fields(resource: &ResourceIr) -> (String, Vec<serde_json::Value>) {
    let mut metadata = super::super::catalog::crud_resource_metadata(resource);
    let fields = metadata
        .as_object_mut()
        .expect("资源元数据必须是对象")
        .remove("fields")
        .expect("资源元数据必须包含字段");
    (
        super::super::catalog::rust_json(&metadata),
        fields.as_array().expect("字段元数据必须是数组").clone(),
    )
}
