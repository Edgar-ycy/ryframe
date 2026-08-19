use crate::{naming, schema::TableInfo, template};

pub fn render_repository(table: &TableInfo, base_name: &str) -> String {
    let struct_name = naming::to_pascal_case(base_name);
    let snake = naming::to_snake_case(base_name);
    let business_primary_keys = template::business_primary_keys(table);
    let key_arguments = business_primary_keys
        .iter()
        .map(|column| {
            format!(
                "        {}: {},",
                naming::safe_field_name(&column.name),
                column.rust_type
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let key_filters = business_primary_keys
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            format!(
                "\n            .filter({snake}::Column::{}.eq({field}.clone()))",
                naming::to_pascal_case(&column.name)
            )
        })
        .collect::<String>();
    let entity_key_filters = business_primary_keys
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            format!(
                "\n            .filter({snake}::Column::{}.eq(entity.{field}.clone()))",
                naming::to_pascal_case(&column.name)
            )
        })
        .collect::<String>();
    let delete_column = table
        .columns
        .iter()
        .find(|column| column.name == "del_flag");
    let updated_at_column = table
        .columns
        .iter()
        .find(|column| column.name == "updated_at");
    let mut scope_filters = vec![format!(".filter({snake}::Column::TenantId.eq(tenant_id))")];
    if let Some(column) = delete_column {
        scope_filters.push(format!(
            ".filter({snake}::Column::DelFlag.eq({}))",
            template::normal_value(column)
        ));
    }
    let find_filters = scope_filters
        .iter()
        .map(|filter| format!("\n            {filter}"))
        .collect::<String>();
    let tenant_delete_filter =
        format!("\n            .filter({snake}::Column::TenantId.eq(tenant_id))");
    let delete_body = if let Some(column) = delete_column {
        let updated_at_expr = updated_at_column
            .filter(|column| column.rust_type.contains("DateTime<Utc>"))
            .map(|_| {
                format!(
                    "\n            .col_expr(\n                {snake}::Column::UpdatedAt,\n                sea_orm::sea_query::Expr::value(chrono::Utc::now()),\n            )"
                )
            })
            .unwrap_or_default();
        format!(
            r#"        let result = {snake}::Entity::update_many()
            .col_expr(
                {snake}::Column::DelFlag,
                sea_orm::sea_query::Expr::value({deleted_value}),
            ){updated_at_expr}
{key_filters}
            .filter({snake}::Column::DelFlag.eq({normal_value})){tenant_delete_filter}
            .exec(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected == 0 {{
            return Err(AppError::NotFound("记录不存在".into()));
        }}"#,
            deleted_value = template::deleted_value(column),
            normal_value = template::normal_value(column),
        )
    } else {
        format!(
            r#"        let result = {snake}::Entity::delete_many()
{key_filters}{tenant_delete_filter}
            .exec(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        if result.rows_affected == 0 {{
            return Err(AppError::NotFound("记录不存在".into()));
        }}"#,
        )
    };

    format!(
        r#"// 此文件由 ryframe-generator v{generator_version} 自动生成。
// tenant-data-boundary: business
use ryframe_adapters::repository::{{PageResult, ValidatedPageQuery}};
use ryframe_db::{{ReadConsistency, entities::{snake}}};
use ryframe_kernel::{{AppError, AppResult}};
use ryframe_tenant_db::TenantDataSession;
use sea_orm::{{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait}};

pub struct {struct_name}Repository;

impl {struct_name}Repository {{
    pub async fn find_by_id(
        &self,
        session: &TenantDataSession,
{key_arguments}
    ) -> AppResult<Option<{snake}::Model>> {{
        let tenant_id = session.tenant_id();
        let selected = session
            .select_read(ReadConsistency::Eventual)
            .await
            .map_err(crate::map_tenant_data_error)?;
        {snake}::Entity::find()
{key_filters}{find_filters}
            .one(&selected.connection)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }}

    pub async fn find_by_page(
        &self,
        session: &TenantDataSession,
        query: ValidatedPageQuery,
    ) -> AppResult<PageResult<{snake}::Model>> {{
        let tenant_id = session.tenant_id();
        let selected = session
            .select_read(ReadConsistency::Eventual)
            .await
            .map_err(crate::map_tenant_data_error)?;
        let select = {snake}::Entity::find(){find_filters};
        ryframe_db::pagination::paginate(&selected.connection, select, &query).await
    }}

    pub async fn insert(
        &self,
        session: &TenantDataSession,
        entity: {snake}::Model,
    ) -> AppResult<{snake}::Model> {{
        let tenant_id = session.tenant_id();
        if entity.tenant_id != tenant_id {{
            return Err(AppError::Authorization("不能写入其他租户的数据".into()));
        }}
        let transaction = session
            .begin_write()
            .await
            .map_err(crate::map_tenant_data_error)?;
        let saved = {snake}::ActiveModel::from(entity)
            .insert(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(saved)
    }}

    pub async fn update(
        &self,
        session: &TenantDataSession,
        entity: {snake}::Model,
    ) -> AppResult<{snake}::Model> {{
        let tenant_id = session.tenant_id();
        if entity.tenant_id != tenant_id {{
            return Err(AppError::Authorization("不能修改其他租户的数据".into()));
        }}
        let transaction = session
            .begin_write()
            .await
            .map_err(crate::map_tenant_data_error)?;
        let exists = {snake}::Entity::find()
{entity_key_filters}{find_filters}
            .one(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
            .is_some();
        if !exists {{
            return Err(AppError::NotFound("记录不存在".into()));
        }}
        let saved = {snake}::ActiveModel::from(entity)
            .reset_all()
            .update(&transaction)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(saved)
    }}

    pub async fn delete(
        &self,
        session: &TenantDataSession,
{key_arguments}
    ) -> AppResult<()> {{
        let tenant_id = session.tenant_id();
        let transaction = session
            .begin_write()
            .await
            .map_err(crate::map_tenant_data_error)?;
{delete_body}
        transaction
            .commit()
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }}
}}
"#,
        generator_version = crate::GENERATOR_VERSION,
    )
}
