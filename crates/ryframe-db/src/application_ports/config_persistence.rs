use std::sync::Arc;

use crate::{
    CacheNamespaceVersionRepository, ConfigFilter as DatabaseConfigFilter, ConfigRepository,
    ControlDatabaseCluster, ReadConsistency, Repository, TenantConfigTransferRepository,
    entities::config,
};
use ryframe_kernel::{ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    AuthorizationCache, ConfigFilter, ConfigPersistencePort, ConfigRecord, ConfigTransaction,
    ControlTransaction, PersistenceFuture,
};

use super::control_transaction::DatabasePortTransaction;

pub fn port(
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
) -> Arc<dyn ConfigPersistencePort> {
    Arc::new(DatabaseConfigPersistence {
        database,
        authorization_cache,
    })
}

struct DatabaseConfigPersistence {
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
}

struct DatabaseConfigTransaction {
    transaction: DatabasePortTransaction,
    authorization_cache: AuthorizationCache,
}

impl ConfigPersistencePort for DatabaseConfigPersistence {
    fn find_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: ConfigFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<ConfigRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            let filter = to_database_filter(filter);
            let result = ConfigRepository
                .find_by_page_filtered(&database, tenant_id, &page, &filter)
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
        filter: ConfigFilter<'a>,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<ConfigRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            ConfigRepository
                .find_for_export_after_id(&database, tenant_id, &to_database_filter(filter), window)
                .await
                .map(|records| records.into_iter().map(to_record).collect())
        })
    }

    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<ConfigRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            Ok(ConfigRepository
                .find_by_id(&database, tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn find_by_key<'a>(
        &'a self,
        tenant_id: &'a str,
        key: &'a str,
    ) -> PersistenceFuture<'a, Option<ConfigRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            Ok(ConfigRepository
                .find_by_key(&database, tenant_id, key)
                .await?
                .map(to_record))
        })
    }

    fn find_namespace_version<'a>(
        &'a self,
        tenant_id: &'a str,
        namespace: &'a str,
    ) -> PersistenceFuture<'a, i64> {
        Box::pin(async move {
            CacheNamespaceVersionRepository
                .find_version(self.database.write(), tenant_id, namespace)
                .await
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ConfigTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseConfigTransaction {
                transaction: transaction.into(),
                authorization_cache: self.authorization_cache.clone(),
            }) as Box<dyn ConfigTransaction>)
        })
    }
}

impl ConfigTransaction for DatabaseConfigTransaction {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
                .await
                .map(|_| ())
        })
    }

    fn find_by_key_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        key: &'a str,
    ) -> PersistenceFuture<'a, Option<ConfigRecord>> {
        Box::pin(async move {
            Ok(config::Entity::find()
                .filter(config::Column::TenantId.eq(tenant_id))
                .filter(config::Column::Key.eq(key))
                .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
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
    ) -> PersistenceFuture<'a, Option<ConfigRecord>> {
        Box::pin(async move {
            Ok(ConfigRepository
                .find_by_id_for_update(&self.transaction, tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: ConfigRecord,
    ) -> PersistenceFuture<'a, ConfigRecord> {
        Box::pin(async move {
            ConfigRepository
                .insert_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
                .await
                .map(to_record)
        })
    }

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: ConfigRecord,
    ) -> PersistenceFuture<'a, ConfigRecord> {
        Box::pin(async move {
            ConfigRepository
                .update_in_transaction(&self.transaction, tenant_id, to_entity(tenant_id, record))
                .await
                .map(to_record)
        })
    }

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            ConfigRepository
                .delete_in_transaction(&self.transaction, tenant_id, id)
                .await
        })
    }

    fn record_namespace_change<'a>(
        &'a self,
        tenant_id: &'a str,
        namespace: &'a str,
    ) -> PersistenceFuture<'a, i64> {
        Box::pin(async move {
            self.authorization_cache
                .record_namespace_version_in_transaction(&self.transaction, tenant_id, namespace)
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

impl ControlTransaction for DatabaseConfigTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move {
            super::audit_persistence::commit_current_audit(self.transaction.into_inner()).await
        })
    }
}

fn to_database_filter(filter: ConfigFilter<'_>) -> DatabaseConfigFilter<'_> {
    DatabaseConfigFilter {
        name: filter.name,
        key: filter.key,
    }
}

fn to_record(model: config::Model) -> ConfigRecord {
    ConfigRecord {
        id: model.id,
        name: model.name,
        key: model.key,
        value: model.value,
        portable: model.portable,
        remark: model.remark,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_entity(tenant_id: &str, record: ConfigRecord) -> config::Model {
    config::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        name: record.name,
        key: record.key,
        value: record.value,
        portable: record.portable,
        remark: record.remark,
        del_flag: config::Model::DEL_FLAG_NORMAL.into(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
