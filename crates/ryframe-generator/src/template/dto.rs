use crate::{naming, schema::TableInfo, template};

pub fn render_dto(table: &TableInfo, base_name: &str) -> String {
    let struct_name = naming::to_pascal_case(base_name);
    let business_primary_keys = template::business_primary_keys(table);
    let key_fields = render_fields(business_primary_keys.iter().copied());
    let key_conversion = business_primary_keys
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            format!("            {field}: value.{field},")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let command_columns = template::command_columns(table).collect::<Vec<_>>();
    let request_fields = render_fields(command_columns.iter().copied());
    let command_conversion = command_columns
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            format!("            {field}: value.{field},")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let chrono_import = template::chrono_import(
        business_primary_keys
            .iter()
            .copied()
            .chain(command_columns.iter().copied()),
    );

    format!(
        r#"// 此文件由 ryframe-generator v{generator_version} 自动生成。
// 租户数据边界：API 数据传输
{chrono_import}use ryframe_application::business::{{
    Create{struct_name}Command, {struct_name}Key, {struct_name}PageQuery,
    Update{struct_name}Command,
}};
use ryframe_kernel::AppResult;
use serde::Deserialize;
use utoipa::{{IntoParams, ToSchema}};

#[derive(Debug, Deserialize, IntoParams, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct {struct_name}KeyDto {{
{key_fields}
}}

#[derive(Debug, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
pub struct {struct_name}ListDto {{
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}}

impl {struct_name}ListDto {{
    pub fn into_query(self, default_page_size: u64, max_page_size: u64) -> AppResult<{struct_name}PageQuery> {{
        {struct_name}PageQuery::new(
            self.page.unwrap_or(1),
            self.page_size.unwrap_or(default_page_size),
            max_page_size,
        )
    }}
}}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Create{struct_name}Dto {{
{request_fields}
}}

#[derive(Debug, Deserialize, validator::Validate, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Update{struct_name}Dto {{
{request_fields}
}}

impl From<{struct_name}KeyDto> for {struct_name}Key {{
    fn from(value: {struct_name}KeyDto) -> Self {{
        Self {{
{key_conversion}
        }}
    }}
}}

impl From<Create{struct_name}Dto> for Create{struct_name}Command {{
    fn from(value: Create{struct_name}Dto) -> Self {{
        Self {{
{command_conversion}
        }}
    }}
}}

impl From<Update{struct_name}Dto> for Update{struct_name}Command {{
    fn from(value: Update{struct_name}Dto) -> Self {{
        Self {{
{command_conversion}
        }}
    }}
}}
"#,
        generator_version = crate::GENERATOR_VERSION,
    )
}

fn render_fields<'a>(columns: impl Iterator<Item = &'a crate::schema::ColumnInfo>) -> String {
    columns
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            let rust_type = &column.rust_type;
            format!("    pub {field}: {rust_type},")
        })
        .collect::<Vec<_>>()
        .join("\n")
}
