use crate::{naming, schema::TableInfo, template};

pub fn render_use_case(table: &TableInfo, base_name: &str) -> String {
    let struct_name = naming::to_pascal_case(base_name);
    let business_primary_keys = template::business_primary_keys(table);
    let key_fields = render_fields(business_primary_keys.iter().copied());
    let record_fields = render_fields(table.columns.iter());
    let command_columns = template::command_columns(table).collect::<Vec<_>>();
    let command_fields = render_fields(command_columns.iter().copied());
    let public_columns = template::public_columns(table).collect::<Vec<_>>();
    let vo_fields = public_columns
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            let rust_type = if column.is_primary_key {
                "String"
            } else {
                column.rust_type.as_str()
            };
            format!("    pub {field}: {rust_type},")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let record_to_vo = public_columns
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            if column.is_primary_key {
                format!("            {field}: record.{field}.to_string(),")
            } else {
                format!("            {field}: record.{field},")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let create_record_fields = table
        .columns
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            let value = if column.name == "tenant_id" {
                "tenant_id.to_owned()".into()
            } else if column.is_primary_key {
                format!("key.{field}")
            } else if column.name == "del_flag" {
                template::normal_value(column)
            } else if is_timestamp_column(&column.name) {
                timestamp_value(column)
            } else if template::is_managed_column(column) {
                "Default::default()".into()
            } else {
                format!("command.{field}")
            };
            format!("            {field}: {value},")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let update_fields = command_columns
        .iter()
        .map(|column| {
            let field = naming::safe_field_name(&column.name);
            format!("            record.{field} = command.{field};")
        })
        .chain(
            table
                .columns
                .iter()
                .filter(|column| matches!(column.name.as_str(), "updated_at" | "update_time"))
                .map(|column| {
                    let field = naming::safe_field_name(&column.name);
                    let value = timestamp_value(column);
                    format!("            record.{field} = {value};")
                }),
        )
        .collect::<Vec<_>>()
        .join("\n");
    let chrono_import = template::chrono_import(table.columns.iter());

    format!(
        r#"// 此文件由 ryframe-generator v{generator_version} 自动生成。
// 租户数据边界：应用用例
{chrono_import}use std::sync::Arc;

use async_trait::async_trait;
use ryframe_kernel::{{ActorContext, AppError, AppResult}};
use serde::Serialize;

#[derive(Debug)]
pub struct {struct_name}Key {{
{key_fields}
}}

#[derive(Debug)]
pub struct {struct_name}Record {{
{record_fields}
}}

#[derive(Debug, Serialize)]
pub struct {struct_name}Vo {{
{vo_fields}
}}

#[derive(Debug)]
pub struct Create{struct_name}Command {{
{command_fields}
}}

#[derive(Debug)]
pub struct Update{struct_name}Command {{
{command_fields}
}}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct {struct_name}PageQuery {{
    page: u64,
    page_size: u64,
}}

impl {struct_name}PageQuery {{
    pub fn new(page: u64, page_size: u64, max_page_size: u64) -> AppResult<Self> {{
        if page == 0 || page_size == 0 || page_size > max_page_size {{
            return Err(AppError::Validation("分页参数超出允许范围".into()));
        }}
        page.checked_sub(1)
            .and_then(|index| index.checked_mul(page_size))
            .ok_or_else(|| AppError::Validation("分页偏移量溢出".into()))?;
        Ok(Self {{ page, page_size }})
    }}

    pub const fn page(self) -> u64 {{
        self.page
    }}

    pub const fn page_size(self) -> u64 {{
        self.page_size
    }}
}}

#[derive(Debug)]
pub struct {struct_name}Page<T> {{
    pub records: Vec<T>,
    pub total: u64,
    pub page: u64,
    pub page_size: u64,
}}

impl<T> {struct_name}Page<T> {{
    pub fn new(records: Vec<T>, total: u64, query: {struct_name}PageQuery) -> Self {{
        Self {{
            records,
            total,
            page: query.page(),
            page_size: query.page_size(),
        }}
    }}
}}

#[async_trait]
pub trait {struct_name}DataSource: Send + Sync {{
    type ReadConnection: Send + Sync;
    type Transaction: Send + Sync;

    async fn read_connection(&self, tenant_id: &str) -> AppResult<Self::ReadConnection>;
    async fn begin(&self, tenant_id: &str) -> AppResult<Self::Transaction>;
    async fn commit(&self, transaction: Self::Transaction) -> AppResult<()>;
    async fn rollback(&self, transaction: Self::Transaction) -> AppResult<()>;
}}

#[async_trait]
pub trait {struct_name}RepositoryPort<C, T>: Send + Sync {{
    async fn find_by_id(
        &self,
        connection: &C,
        tenant_id: &str,
        key: &{struct_name}Key,
    ) -> AppResult<Option<{struct_name}Record>>;

    async fn find_by_id_for_update(
        &self,
        transaction: &T,
        tenant_id: &str,
        key: &{struct_name}Key,
    ) -> AppResult<Option<{struct_name}Record>>;

    async fn find_by_page(
        &self,
        connection: &C,
        tenant_id: &str,
        query: &{struct_name}PageQuery,
    ) -> AppResult<{struct_name}Page<{struct_name}Record>>;

    async fn insert(
        &self,
        transaction: &T,
        tenant_id: &str,
        record: {struct_name}Record,
    ) -> AppResult<{struct_name}Record>;

    async fn update(
        &self,
        transaction: &T,
        tenant_id: &str,
        record: {struct_name}Record,
    ) -> AppResult<{struct_name}Record>;

    async fn delete(
        &self,
        transaction: &T,
        tenant_id: &str,
        key: &{struct_name}Key,
    ) -> AppResult<()>;
}}

pub trait {struct_name}KeyGenerator: Send + Sync {{
    fn next_key(&self) -> AppResult<{struct_name}Key>;
}}

pub struct {struct_name}UseCase<D, R, I> {{
    data_source: Arc<D>,
    repository: Arc<R>,
    key_generator: Arc<I>,
}}

impl<D, R, I> {struct_name}UseCase<D, R, I>
where
    D: {struct_name}DataSource,
    R: {struct_name}RepositoryPort<D::ReadConnection, D::Transaction>,
    I: {struct_name}KeyGenerator,
{{
    pub fn new(data_source: Arc<D>, repository: Arc<R>, key_generator: Arc<I>) -> Self {{
        Self {{
            data_source,
            repository,
            key_generator,
        }}
    }}

    pub async fn find_by_page(
        &self,
        actor: &ActorContext,
        query: {struct_name}PageQuery,
    ) -> AppResult<{struct_name}Page<{struct_name}Vo>> {{
        let tenant_id = validated_tenant_id(actor)?;
        let connection = self.data_source.read_connection(tenant_id).await?;
        let page = self
            .repository
            .find_by_page(&connection, tenant_id, &query)
            .await?;
        Ok({struct_name}Page::new(
            page.records.into_iter().map({struct_name}Vo::from).collect(),
            page.total,
            query,
        ))
    }}

    pub async fn find_by_id(
        &self,
        actor: &ActorContext,
        key: &{struct_name}Key,
    ) -> AppResult<Option<{struct_name}Vo>> {{
        let tenant_id = validated_tenant_id(actor)?;
        let connection = self.data_source.read_connection(tenant_id).await?;
        Ok(self
            .repository
            .find_by_id(&connection, tenant_id, key)
            .await?
            .map({struct_name}Vo::from))
    }}

    pub async fn create(
        &self,
        actor: &ActorContext,
        command: Create{struct_name}Command,
    ) -> AppResult<{struct_name}Vo> {{
        let tenant_id = validated_tenant_id(actor)?;
        let key = self.key_generator.next_key()?;
        let record = {struct_name}Record {{
{create_record_fields}
        }};
        let transaction = self.data_source.begin(tenant_id).await?;
        let result = self
            .repository
            .insert(&transaction, tenant_id, record)
            .await
            .map({struct_name}Vo::from);
        self.finish(transaction, result).await
    }}

    pub async fn update(
        &self,
        actor: &ActorContext,
        key: &{struct_name}Key,
        command: Update{struct_name}Command,
    ) -> AppResult<{struct_name}Vo> {{
        let tenant_id = validated_tenant_id(actor)?;
        let transaction = self.data_source.begin(tenant_id).await?;
        let result = async {{
            let mut record = self
                .repository
                .find_by_id_for_update(&transaction, tenant_id, key)
                .await?
                .ok_or_else(|| AppError::NotFound("记录不存在".into()))?;
{update_fields}
            self.repository
                .update(&transaction, tenant_id, record)
                .await
                .map({struct_name}Vo::from)
        }}
        .await;
        self.finish(transaction, result).await
    }}

    pub async fn delete(
        &self,
        actor: &ActorContext,
        key: &{struct_name}Key,
    ) -> AppResult<()> {{
        let tenant_id = validated_tenant_id(actor)?;
        let transaction = self.data_source.begin(tenant_id).await?;
        let result = self
            .repository
            .delete(&transaction, tenant_id, key)
            .await;
        self.finish(transaction, result).await
    }}

    async fn finish<T>(&self, transaction: D::Transaction, result: AppResult<T>) -> AppResult<T> {{
        match result {{
            Ok(value) => {{
                self.data_source.commit(transaction).await?;
                Ok(value)
            }}
            Err(error) => {{
                if self.data_source.rollback(transaction).await.is_err() {{
                    return Err(AppError::Internal("业务事务回滚失败".into()));
                }}
                Err(error)
            }}
        }}
    }}
}}

impl From<{struct_name}Record> for {struct_name}Vo {{
    fn from(record: {struct_name}Record) -> Self {{
        Self {{
{record_to_vo}
        }}
    }}
}}

fn validated_tenant_id(actor: &ActorContext) -> AppResult<&str> {{
    if actor.tenant_id.is_empty() || actor.tenant_id.trim() != actor.tenant_id {{
        return Err(AppError::Validation("租户标识无效".into()));
    }}
    Ok(&actor.tenant_id)
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

fn is_timestamp_column(name: &str) -> bool {
    matches!(
        name,
        "created_at" | "updated_at" | "create_time" | "update_time"
    )
}

fn timestamp_value(column: &crate::schema::ColumnInfo) -> String {
    if column.rust_type == "DateTime<Utc>" {
        "Utc::now()".into()
    } else if column.rust_type == "Option<DateTime<Utc>>" {
        "Some(Utc::now())".into()
    } else {
        "Default::default()".into()
    }
}
