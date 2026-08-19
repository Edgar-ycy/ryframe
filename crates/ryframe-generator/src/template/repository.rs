use crate::{naming, schema::TableInfo, template};

pub fn render_repository(table: &TableInfo, base_name: &str) -> String {
    let struct_name = naming::to_pascal_case(base_name);
    let snake = naming::to_snake_case(base_name);
    let business_primary_keys = template::business_primary_keys(table);
    let key_filters = business_primary_keys
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            let value = borrowed_key_value(column, &field);
            format!(
                "\n            .filter({snake}::Column::{}.eq({value}))",
                naming::to_pascal_case(&column.name)
            )
        })
        .collect::<String>();
    let key_order = business_primary_keys
        .iter()
        .map(|column| {
            format!(
                "\n            .order_by_asc({snake}::Column::{})",
                naming::to_pascal_case(&column.name)
            )
        })
        .collect::<String>();
    let soft_delete = table
        .columns
        .iter()
        .find(|column| column.name == "del_flag");
    let active_filter = soft_delete
        .map(|column| {
            format!(
                "\n            .filter({snake}::Column::DelFlag.eq({}))",
                template::normal_value(column)
            )
        })
        .unwrap_or_default();
    let delete_body = render_delete_body(table, &snake, &key_filters, soft_delete);
    let record_fields = table
        .columns
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            format!("        {field}: model.{field},")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let model_fields = table
        .columns
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            format!("        {field}: record.{field},")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let chrono_import = table
        .columns
        .iter()
        .find(|column| {
            matches!(column.name.as_str(), "updated_at" | "update_time")
                && column.rust_type.contains("DateTime<Utc>")
        })
        .map(|_| "use chrono::Utc;\n")
        .unwrap_or_default();

    format!(
        r#"// 此文件由 ryframe-generator v{generator_version} 自动生成。
// 租户数据边界：SQL 适配器
{chrono_import}use async_trait::async_trait;
use ryframe_application::business::{{
    {struct_name}Key, {struct_name}Page, {struct_name}PageQuery, {struct_name}Record,
    {struct_name}RepositoryPort,
}};
use ryframe_kernel::{{AppError, AppResult}};
use sea_orm::{{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DatabaseTransaction, DbErr, EntityTrait,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
}};

use crate::entities::{snake};

pub struct {struct_name}Repository;

#[async_trait]
impl {struct_name}RepositoryPort<DatabaseConnection, DatabaseTransaction>
    for {struct_name}Repository
{{
    async fn find_by_id(
        &self,
        connection: &DatabaseConnection,
        tenant_id: &str,
        key: &{struct_name}Key,
    ) -> AppResult<Option<{struct_name}Record>> {{
        {snake}::Entity::find()
            .filter({snake}::Column::TenantId.eq(tenant_id)){key_filters}{active_filter}
            .one(connection)
            .await
            .map(|record| record.map(into_record))
            .map_err(database_error)
    }}

    async fn find_by_id_for_update(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        key: &{struct_name}Key,
    ) -> AppResult<Option<{struct_name}Record>> {{
        {snake}::Entity::find()
            .filter({snake}::Column::TenantId.eq(tenant_id)){key_filters}{active_filter}
            .lock_exclusive()
            .one(transaction)
            .await
            .map(|record| record.map(into_record))
            .map_err(database_error)
    }}

    async fn find_by_page(
        &self,
        connection: &DatabaseConnection,
        tenant_id: &str,
        query: &{struct_name}PageQuery,
    ) -> AppResult<{struct_name}Page<{struct_name}Record>> {{
        let paginator = {snake}::Entity::find()
            .filter({snake}::Column::TenantId.eq(tenant_id)){active_filter}{key_order}
            .paginate(connection, query.page_size());
        let total = paginator.num_items().await.map_err(database_error)?;
        let records = paginator
            .fetch_page(query.page() - 1)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(into_record)
            .collect();
        Ok({struct_name}Page::new(records, total, *query))
    }}

    async fn insert(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        record: {struct_name}Record,
    ) -> AppResult<{struct_name}Record> {{
        ensure_tenant(&record, tenant_id)?;
        {snake}::ActiveModel::from(into_model(record))
            .insert(transaction)
            .await
            .map(into_record)
            .map_err(database_error)
    }}

    async fn update(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        record: {struct_name}Record,
    ) -> AppResult<{struct_name}Record> {{
        ensure_tenant(&record, tenant_id)?;
        {snake}::ActiveModel::from(into_model(record))
            .reset_all()
            .update(transaction)
            .await
            .map(into_record)
            .map_err(write_error)
    }}

    async fn delete(
        &self,
        transaction: &DatabaseTransaction,
        tenant_id: &str,
        key: &{struct_name}Key,
    ) -> AppResult<()> {{
{delete_body}
        if result.rows_affected == 0 {{
            return Err(AppError::NotFound("记录不存在".into()));
        }}
        Ok(())
    }}
}}

fn ensure_tenant(record: &{struct_name}Record, tenant_id: &str) -> AppResult<()> {{
    if record.tenant_id != tenant_id {{
        return Err(AppError::Authorization("不能写入其他租户的数据".into()));
    }}
    Ok(())
}}

fn into_record(model: {snake}::Model) -> {struct_name}Record {{
    {struct_name}Record {{
{record_fields}
    }}
}}

fn into_model(record: {struct_name}Record) -> {snake}::Model {{
    {snake}::Model {{
{model_fields}
    }}
}}

fn write_error(error: DbErr) -> AppError {{
    match error {{
        DbErr::RecordNotFound(_) => AppError::NotFound("记录不存在".into()),
        other => database_error(other),
    }}
}}

fn database_error(error: DbErr) -> AppError {{
    AppError::Database(error.to_string())
}}
"#,
        generator_version = crate::GENERATOR_VERSION,
    )
}

fn borrowed_key_value(column: &crate::schema::ColumnInfo, field: &str) -> String {
    match column.rust_type.as_str() {
        "String" => format!("key.{field}.as_str()"),
        "Vec<u8>" => format!("key.{field}.as_slice()"),
        "Option<String>" | "Option<Vec<u8>>" => format!("key.{field}.as_deref()"),
        _ => format!("key.{field}"),
    }
}

fn render_delete_body(
    table: &TableInfo,
    snake: &str,
    key_filters: &str,
    soft_delete: Option<&crate::schema::ColumnInfo>,
) -> String {
    let tenant_filter = format!("\n            .filter({snake}::Column::TenantId.eq(tenant_id))");
    if let Some(column) = soft_delete {
        let updated_at = table
            .columns
            .iter()
            .find(|column| matches!(column.name.as_str(), "updated_at" | "update_time"));
        let updated_at_expr = updated_at
            .filter(|column| column.rust_type.contains("DateTime<Utc>"))
            .map(|column| {
                format!(
                    "\n            .col_expr(\n                {snake}::Column::{},\n                sea_orm::sea_query::Expr::value(Utc::now()),\n            )",
                    naming::to_pascal_case(&column.name)
                )
            })
            .unwrap_or_default();
        format!(
            r#"        let result = {snake}::Entity::update_many()
            .col_expr(
                {snake}::Column::DelFlag,
                sea_orm::sea_query::Expr::value({deleted_value}),
            ){updated_at_expr}{key_filters}{tenant_filter}
            .filter({snake}::Column::DelFlag.eq({normal_value}))
            .exec(transaction)
            .await
            .map_err(database_error)?;"#,
            deleted_value = template::deleted_value(column),
            normal_value = template::normal_value(column),
        )
    } else {
        format!(
            r#"        let result = {snake}::Entity::delete_many(){key_filters}{tenant_filter}
            .exec(transaction)
            .await
            .map_err(database_error)?;"#,
        )
    }
}
