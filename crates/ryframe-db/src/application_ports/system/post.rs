use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, PostFilter as DatabasePostFilter, PostRepository, ReadConsistency,
    Repository, TenantConfigTransferRepository, entities::post,
};
use ryframe_kernel::{AppError, ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    ControlTransaction, PersistenceFuture,
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

impl PostPersistencePort for DatabasePostPersistence {
    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<PostRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            Ok(PostRepository
                .find_by_id(&database, tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: PostFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<PostRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: PostFilter<'a>,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<PostRecord>> {
        Box::pin(async move {
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
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn PostTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabasePostTransaction { transaction }) as Box<dyn PostTransaction>)
        })
    }
}

impl PostTransaction for DatabasePostTransaction {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
                .await
                .map(|_| ())
        })
    }

    fn find_by_code_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, Option<PostRecord>> {
        Box::pin(async move {
            Ok(post::Entity::find()
                .filter(post::Column::TenantId.eq(tenant_id))
                .filter(post::Column::Code.eq(code))
                .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_record))
        })
    }

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<PostRecord>> {
        Box::pin(async move {
            Ok(post::Entity::find_by_id(id)
                .filter(post::Column::TenantId.eq(tenant_id))
                .filter(post::Column::DelFlag.eq(post::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_record))
        })
    }

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: PostRecord,
    ) -> PersistenceFuture<'a, PostRecord> {
        Box::pin(async move {
            PostRepository
                .insert_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
                .await
                .map(to_record)
        })
    }

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: PostRecord,
    ) -> PersistenceFuture<'a, PostRecord> {
        Box::pin(async move {
            PostRepository
                .update_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
                .await
                .map(to_record)
        })
    }

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            PostRepository
                .delete_in_transaction(&self.transaction, tenant_id, id)
                .await
        })
    }

    fn increment_configuration_version<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .increment_configuration_version_in_txn(&self.transaction, tenant_id)
                .await
                .map(|_| ())
        })
    }
}

impl ControlTransaction for DatabasePostTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { super::super::audit::commit_current_audit(self.transaction).await })
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
