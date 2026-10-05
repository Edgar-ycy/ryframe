{header}use std::sync::Arc;

use async_trait::async_trait;
use ryframe_application::generated::{name}::{{
    {pascal}Filter, {pascal}PersistencePort, {pascal}Record{detail_import}, {pascal}Transaction,
}};
use ryframe_application::{{PersistenceTransaction, TransactionAuditMode}};
use ryframe_kernel::{{AppError, AppResult, PageResult, ValidatedPageQuery}};
use sea_orm::{{
    ActiveModelTrait, ColumnTrait, DatabaseTransaction, EntityTrait,
    {model_trait}QueryFilter, QueryOrder, QuerySelect, {transaction_trait}sea_query::LockType,
}};

use crate::DbResultExt;
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
    ) -> AppResult<Option<{detail_type}>> {{
{read_connection}
{detail_read}
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
            .db()?
            .map(to_record))
    }}

    async fn insert(&self, record: {pascal}Record) -> AppResult<{pascal}Record> {{
        self.ensure_tenant(&record.tenant_id)?;
        entity::ActiveModel::from(to_entity(record))
            .insert(&self.transaction)
            .await
            .map(to_record)
            .db_conflicts(&[{unique_conflicts}], {generic_conflict:?})
    }}

    async fn update(&self, record: {pascal}Record) -> AppResult<{pascal}Record> {{
        self.ensure_tenant(&record.tenant_id)?;
        entity::ActiveModel::from(to_entity(record))
            .reset_all()
            .update(&self.transaction)
            .await
            .map(to_record)
            .db_conflicts(&[{unique_conflicts}], {generic_conflict:?})
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
        self.transaction.rollback().await.db()
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

{relation_mappers}

{tenant_error}
