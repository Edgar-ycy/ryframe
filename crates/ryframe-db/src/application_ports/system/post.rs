use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, PostFilter as DatabasePostFilter, PostRepository, ReadConsistency,
    Repository, TenantConfigTransferRepository, entities::post,
};
use async_trait::async_trait;
use ryframe_kernel::{AppError, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode,
    ports::system::{PostFilter, PostPersistencePort, PostRecord, PostTransaction},
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn PostPersistencePort> {
    Arc::new(DatabasePostPersistence { database })
}

struct DatabasePostPersistence {
    database: ControlDatabaseCluster,
}

struct DatabasePostTransaction {
    transaction: sea_orm::DatabaseTransaction,
}

#[async_trait]
impl PostPersistencePort for DatabasePostPersistence {
    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<PostRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        Ok(PostRepository
            .find_by_id(&database, tenant_id, id)
            .await?
            .map(to_record))
    }

    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: PostFilter<'_>,
    ) -> AppResult<PageResult<PostRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        let result = PostRepository
            .find_by_page_filtered(
                &database,
                tenant_id,
                page,
                filter.name,
                filter.code,
                filter.status,
            )
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(to_record).collect(),
            result.total,
            &page,
        ))
    }

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: PostFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<PostRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        let filter = DatabasePostFilter {
            name: filter.name,
            code: filter.code,
            status: filter.status,
        };
        Ok(PostRepository
            .find_for_export_after_id(&database, tenant_id, &filter, window)
            .await?
            .into_iter()
            .map(to_record)
            .collect())
    }

    async fn begin(&self) -> AppResult<Box<dyn PostTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabasePostTransaction { transaction }) as Box<dyn PostTransaction>)
    }
}

#[async_trait]
impl PostTransaction for DatabasePostTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()> {
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
            .await
            .map(|_| ())
    }

    async fn find_by_code_for_update(
        &self,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<PostRecord>> {
        Ok(post::Entity::find()
            .filter(post::Column::TenantId.eq(tenant_id))
            .filter(post::Column::Code.eq(code))
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map_err(database_error)?
            .map(to_record))
    }

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<PostRecord>> {
        Ok(post::Entity::find_by_id(id)
            .filter(post::Column::TenantId.eq(tenant_id))
            .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map_err(database_error)?
            .map(to_record))
    }

    async fn insert(&self, tenant_id: &str, record: PostRecord) -> AppResult<PostRecord> {
        PostRepository
            .insert_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
            .await
            .map(to_record)
    }

    async fn update(&self, tenant_id: &str, record: PostRecord) -> AppResult<PostRecord> {
        PostRepository
            .update_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
            .await
            .map(to_record)
    }

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        PostRepository
            .delete_in_transaction(&self.transaction, tenant_id, id)
            .await
    }

    async fn increment_configuration_version(&self, tenant_id: &str) -> AppResult<()> {
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&self.transaction, tenant_id)
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl PersistenceTransaction for DatabasePostTransaction {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => {
                super::super::audit::commit_current_audit(self.transaction).await
            }
            TransactionAuditMode::Skip => self.transaction.commit().await.map_err(database_error),
        }
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
    }
}

fn to_record(model: post::Model) -> PostRecord {
    PostRecord {
        id: model.id,
        name: model.name,
        code: model.code,
        sort: model.sort,
        status: model.status,
        remark: model.remark,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_entity(tenant_id: &str, record: PostRecord) -> post::Model {
    post::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        name: record.name,
        code: record.code,
        sort: record.sort,
        status: record.status,
        remark: record.remark,
        del_flag: post::Model::DEL_FLAG_NORMAL.into(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}
