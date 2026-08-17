use crate::{naming, schema::TableInfo, template};

pub fn render_service(table: &TableInfo, base_name: &str) -> String {
    let struct_name = naming::to_pascal_case(base_name);
    let snake = naming::to_snake_case(base_name);
    let repository_field = format!("{}_repo", snake);
    let business_primary_keys = template::business_primary_keys(table);
    let key_fields = business_primary_keys
        .iter()
        .map(|column| {
            format!(
                "    pub {}: {},",
                naming::safe_field_name(&column.name),
                column.rust_type
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let key_call_arguments = business_primary_keys
        .iter()
        .map(|column| format!("key.{}", naming::safe_field_name(&column.name)))
        .collect::<Vec<_>>()
        .join(", ");
    let public_columns = template::public_columns(table).collect::<Vec<_>>();
    let command_columns = template::command_columns(table).collect::<Vec<_>>();

    let vo_fields = public_columns
        .iter()
        .map(|column| {
            let field_name = naming::safe_field_name(&column.name);
            let rust_type = if column.is_primary_key {
                "String"
            } else {
                column.rust_type.as_str()
            };
            format!("    pub {}: {},", field_name, rust_type)
        })
        .collect::<Vec<_>>()
        .join("\n");
    let model_to_vo_fields = public_columns
        .iter()
        .map(|column| {
            let field_name = naming::safe_field_name(&column.name);
            if column.is_primary_key {
                format!("            {field_name}: model.{field_name}.to_string(),")
            } else {
                format!("            {field_name}: model.{field_name},")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let command_fields = command_columns
        .iter()
        .map(|column| {
            format!(
                "    pub {}: {},",
                naming::safe_field_name(&column.name),
                column.rust_type
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let create_model_fields = table
        .columns
        .iter()
        .map(|column| {
            let field_name = naming::safe_field_name(&column.name);
            let value = if column.name == "tenant_id" {
                "tenant_id.to_owned()".into()
            } else if column.is_primary_key {
                if column.is_auto_increment {
                    "Default::default()".into()
                } else if column.rust_type == "i64" {
                    "snowflake::try_next_snowflake_id()?".into()
                } else {
                    "Default::default()".into()
                }
            } else {
                match column.name.as_str() {
                    "del_flag" => template::normal_value(column),
                    "created_at" | "updated_at" | "create_time" | "update_time" => {
                        "Default::default()".into()
                    }
                    _ => format!("command.{field_name}"),
                }
            };
            format!("            {field_name}: {value},")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let update_fields = command_columns
        .iter()
        .map(|column| {
            let field_name = naming::safe_field_name(&column.name);
            format!("        model.{field_name} = command.{field_name};")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let common_import = if business_primary_keys
        .iter()
        .any(|column| !column.is_auto_increment && column.rust_type == "i64")
    {
        "use ryframe_kernel::{ActorContext, AppError, AppResult};\nuse ryframe_utils::snowflake;"
    } else {
        "use ryframe_kernel::{ActorContext, AppError, AppResult};"
    };
    let chrono_import = template::chrono_import(table.columns.iter());

    format!(
        r#"// 此文件由 ryframe-generator v{generator_version} 自动生成。
// tenant-data-boundary: business
{chrono_import}{common_import}
use std::sync::Arc;

use ryframe_core::{{
    auto_fill::{{AutoFill, FillContext}},
    repository::{{ValidatedPageQuery, PageResult}},
}};
use crate::business::{struct_name}Repository;
use ryframe_db::entities::{snake};
use ryframe_tenant_db::TenantDatabaseRouter;
use serde::{{Deserialize, Serialize}};

#[derive(Clone, Debug, Deserialize)]
pub struct {struct_name}Key {{
{key_fields}
}}

#[derive(Debug, Serialize)]
pub struct {struct_name}Vo {{
{vo_fields}
}}

pub struct Create{struct_name}Command {{
{command_fields}
}}

pub struct Update{struct_name}Command {{
{command_fields}
}}

pub struct {struct_name}Service {{
    tenant_data: Arc<TenantDatabaseRouter>,
    {repository_field}: {struct_name}Repository,
}}

impl {struct_name}Service {{
    pub fn new(tenant_data: Arc<TenantDatabaseRouter>) -> Self {{
        Self {{
            tenant_data,
            {repository_field}: {struct_name}Repository,
        }}
    }}

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<{struct_name}Vo>> {{
        let tenant_id = crate::validated_tenant_id(actor)?;
        let session = self
            .tenant_data
            .resolve(tenant_id)
            .await
            .map_err(crate::map_tenant_data_error)?;
        let page = self
            .{repository_field}
            .find_by_page(&session, query.clone())
            .await?;
        let records = page.records.into_iter().map({struct_name}Vo::from).collect();
        Ok(PageResult::new(records, page.total, &query))
    }}

    pub async fn find_by_id(
        &self,
        actor: &ActorContext,
        key: {struct_name}Key,
    ) -> AppResult<Option<{struct_name}Vo>> {{
        let tenant_id = crate::validated_tenant_id(actor)?;
        let session = self
            .tenant_data
            .resolve(tenant_id)
            .await
            .map_err(crate::map_tenant_data_error)?;
        Ok(self
            .{repository_field}
            .find_by_id(&session, {key_call_arguments})
            .await?
            .map({struct_name}Vo::from))
    }}

    pub async fn create(
        &self,
        actor: &ActorContext,
        command: Create{struct_name}Command,
    ) -> AppResult<{struct_name}Vo> {{
        let tenant_id = crate::validated_tenant_id(actor)?;
        let session = self
            .tenant_data
            .resolve(tenant_id)
            .await
            .map_err(crate::map_tenant_data_error)?;
        let mut model = {snake}::Model {{
{create_model_fields}
        }};
        model.fill_on_insert(&FillContext::new())?;
        let saved = self
            .{repository_field}
            .insert(&session, model)
            .await?;
        Ok({struct_name}Vo::from(saved))
    }}

    pub async fn update(
        &self,
        actor: &ActorContext,
        key: {struct_name}Key,
        command: Update{struct_name}Command,
    ) -> AppResult<{struct_name}Vo> {{
        let tenant_id = crate::validated_tenant_id(actor)?;
        let session = self
            .tenant_data
            .resolve(tenant_id)
            .await
            .map_err(crate::map_tenant_data_error)?;
        let mut model = self
            .{repository_field}
            .find_by_id(&session, {key_call_arguments})
            .await?
            .ok_or_else(|| AppError::NotFound("记录不存在".into()))?;
{update_fields}
        model.fill_on_update(&FillContext::new())?;
        let saved = self
            .{repository_field}
            .update(&session, model)
            .await?;
        Ok({struct_name}Vo::from(saved))
    }}

    pub async fn delete(
        &self,
        actor: &ActorContext,
        key: {struct_name}Key,
    ) -> AppResult<()> {{
        let tenant_id = crate::validated_tenant_id(actor)?;
        let session = self
            .tenant_data
            .resolve(tenant_id)
            .await
            .map_err(crate::map_tenant_data_error)?;
        self.{repository_field}
            .delete(&session, {key_call_arguments})
            .await
    }}
}}

impl From<{snake}::Model> for {struct_name}Vo {{
    fn from(model: {snake}::Model) -> Self {{
        Self {{
{model_to_vo_fields}
        }}
    }}
}}
"#,
        generator_version = crate::GENERATOR_VERSION,
    )
}
