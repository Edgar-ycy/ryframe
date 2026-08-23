use super::super::{ResourceIr, StorageKind};
use super::queries::*;

pub(crate) fn repository(resource: &ResourceIr, header: &str) -> String {
    let pascal = &resource.pascal_name;
    let name = &resource.name;
    let persistence = persistence_parts(resource);
    let base_select = base_select(resource);
    let id_query = id_query(resource, "id");
    let filters = filter_statements(resource);
    let order = order_statements(resource);
    let to_record = mapping(resource, "model");
    let to_entity = mapping(resource, "record");
    let delete = delete_body(resource);
    let commit = commit_body(resource.storage);
    let unique_conflicts = unique_conflict_cases(resource);
    let generic_conflict = format!("{}已存在", resource.labels.zh_cn);
    let tenant_mismatch = format!("{}事务租户不匹配", resource.labels.zh_cn);
    let tenant_error = if resource.storage == StorageKind::TenantData {
        tenant_error_mapper()
    } else {
        String::new()
    };
    let pagination = match resource.storage {
        StorageKind::ControlRow => "crate::pagination::paginate",
        StorageKind::TenantData => "ryframe_db::pagination::paginate",
    };
    let model_trait = if resource.soft_delete.is_none() {
        "ModelTrait, "
    } else {
        ""
    };
    let transaction_trait = if resource.storage == StorageKind::ControlRow {
        "TransactionTrait, "
    } else {
        ""
    };
    let control_transaction_methods = control_transaction_methods(resource);
    format!(
        r#"{header}use std::sync::Arc;

use async_trait::async_trait;
use ryframe_application::generated::{name}::{{
    {pascal}Filter, {pascal}PersistencePort, {pascal}Record, {pascal}Transaction,
}};
use ryframe_application::{{PersistenceTransaction, TransactionAuditMode}};
use ryframe_kernel::{{AppError, AppResult, PageResult, ValidatedPageQuery}};
use sea_orm::{{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseTransaction, EntityTrait,
    {model_trait}QueryFilter, QueryOrder, QuerySelect, {transaction_trait}sea_query::LockType,
}};

use super::entity;

{declaration}

impl Database{pascal}Transaction {{
    fn ensure_tenant(&self, tenant_id: &str) -> AppResult<()> {{
        if self.tenant_id == tenant_id {{
            Ok(())
        }} else {{
            Err(AppError::Authorization({tenant_mismatch:?}.into()))
        }}
    }}
}}

#[async_trait]
impl {pascal}PersistencePort for Database{pascal}Persistence {{
    async fn find_by_id(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<{pascal}Record>> {{
{read_connection}
        Ok({id_query}
            .one(&database)
            .await
            .map_err(database_error)?
            .map(to_record))
    }}

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: {pascal}Filter<'_>,
    ) -> AppResult<PageResult<{pascal}Record>> {{
{read_connection}
{base_select}
{filters}
{order}
        let result = {pagination}(&database, select, &page).await?;
        Ok(PageResult::new(
            result.records.into_iter().map(to_record).collect(),
            result.total,
            &page,
        ))
    }}

    async fn begin(&self, tenant_id: &str) -> AppResult<Box<dyn {pascal}Transaction>> {{
{begin_transaction}
        Ok(Box::new(Database{pascal}Transaction {{
            tenant_id: tenant_id.to_owned(),
            transaction,
        }}))
    }}
}}

#[async_trait]
impl {pascal}Transaction for Database{pascal}Transaction {{
{control_transaction_methods}
    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<{pascal}Record>> {{
        self.ensure_tenant(tenant_id)?;
        Ok({id_query}
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map_err(database_error)?
            .map(to_record))
    }}

    async fn insert(&self, record: {pascal}Record) -> AppResult<{pascal}Record> {{
        self.ensure_tenant(&record.tenant_id)?;
        entity::ActiveModel::from(to_entity(record))
            .insert(&self.transaction)
            .await
            .map(to_record)
            .map_err(database_error)
    }}

    async fn update(&self, record: {pascal}Record) -> AppResult<{pascal}Record> {{
        self.ensure_tenant(&record.tenant_id)?;
        entity::ActiveModel::from(to_entity(record))
            .update(&self.transaction)
            .await
            .map(to_record)
            .map_err(database_error)
    }}

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()> {{
        self.ensure_tenant(tenant_id)?;
{delete}
    }}
}}

#[async_trait]
impl PersistenceTransaction for Database{pascal}Transaction {{
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {{
{commit}
    }}

    async fn rollback(self: Box<Self>) -> AppResult<()> {{
        self.transaction.rollback().await.map_err(database_error)
    }}
}}

fn to_record(model: entity::Model) -> {pascal}Record {{
    {pascal}Record {{
{to_record}
    }}
}}

fn to_entity(record: {pascal}Record) -> entity::Model {{
    entity::Model {{
{to_entity}
    }}
}}

fn database_error(error: sea_orm::DbErr) -> AppError {{
    let message = error.to_string();
    let normalized = message.to_ascii_lowercase();
    if normalized.contains("1062") || normalized.contains("duplicate entry") {{
{unique_conflicts}
        return AppError::Conflict({generic_conflict:?}.into());
    }}
    AppError::Database(message)
}}

{tenant_error}"#,
        declaration = persistence.declaration,
        read_connection = persistence.read_connection,
        begin_transaction = persistence.begin_transaction,
        pagination = pagination,
        generic_conflict = generic_conflict,
    )
}
