use std::sync::Arc;

use crate::{
    CacheNamespaceVersionRepository, ConfigFilter as DatabaseConfigFilter, ConfigRepository,
    ControlDatabaseCluster, DbResultExt, ReadConsistency, Repository,
    TenantConfigTransferRepository, entities::config, repositories::utc_time_from_chrono,
};
use async_trait::async_trait;
use ryframe_kernel::{AppError, AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    AuthorizationCache, PersistenceTransaction, TransactionAuditMode,
    ports::system::{ConfigFilter, ConfigPersistencePort, ConfigRecord, ConfigTransaction},
};

use super::super::transaction::DatabasePortTransaction;

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

#[async_trait]
impl ConfigPersistencePort for DatabaseConfigPersistence {
    async fn find_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: ConfigFilter<'_>,
    ) -> AppResult<PageResult<ConfigRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        let filter = to_database_filter(filter);
        let result = ConfigRepository
            .find_by_page_filtered(&database, tenant_id, &page, &filter)
            .await?;
        Ok(PageResult::new(
            result
                .records
                .into_iter()
                .map(to_record)
                .collect::<AppResult<_>>()?,
            result.total,
            &page,
        ))
    }

    async fn find_export_batch(
        &self,
        tenant_id: &str,
        filter: ConfigFilter<'_>,
        window: ExportCursorWindow<'_>,
    ) -> AppResult<Vec<ConfigRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        ConfigRepository
            .find_for_export_after_id(&database, tenant_id, &to_database_filter(filter), window)
            .await?
            .into_iter()
            .map(to_record)
            .collect()
    }

    async fn find_by_id(&self, tenant_id: &str, id: i64) -> AppResult<Option<ConfigRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        ConfigRepository
            .find_by_id(&database, tenant_id, id)
            .await?
            .map(to_record)
            .transpose()
    }

    async fn find_by_key(&self, tenant_id: &str, key: &str) -> AppResult<Option<ConfigRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        ConfigRepository
            .find_by_key(&database, tenant_id, key)
            .await?
            .map(to_record)
            .transpose()
    }

    async fn find_namespace_version(&self, tenant_id: &str, namespace: &str) -> AppResult<i64> {
        CacheNamespaceVersionRepository
            .find_version(self.database.write(), tenant_id, namespace)
            .await
    }

    async fn begin(&self) -> AppResult<Box<dyn ConfigTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseConfigTransaction {
            transaction: transaction.into(),
            authorization_cache: self.authorization_cache.clone(),
        }) as Box<dyn ConfigTransaction>)
    }
}

#[async_trait]
impl ConfigTransaction for DatabaseConfigTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()> {
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
            .await
            .map(|_| ())
    }

    async fn find_by_key_for_update(
        &self,
        tenant_id: &str,
        key: &str,
    ) -> AppResult<Option<ConfigRecord>> {
        config::Entity::find()
            .filter(config::Column::TenantId.eq(tenant_id))
            .filter(config::Column::Key.eq(key))
            .filter(config::Column::DelFlag.eq(config::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()?
            .map(to_record)
            .transpose()
    }

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<ConfigRecord>> {
        ConfigRepository
            .find_by_id_for_update(&self.transaction, tenant_id, id)
            .await?
            .map(to_record)
            .transpose()
    }

    async fn insert(&self, tenant_id: &str, record: ConfigRecord) -> AppResult<ConfigRecord> {
        let entity = to_entity(tenant_id, record)?;
        let saved = ConfigRepository
            .insert_in_transaction(&self.transaction, tenant_id, entity)
            .await?;
        to_record(saved)
    }

    async fn update(&self, tenant_id: &str, record: ConfigRecord) -> AppResult<ConfigRecord> {
        let entity = to_entity(tenant_id, record)?;
        let saved = ConfigRepository
            .update_in_transaction(&self.transaction, tenant_id, entity)
            .await?;
        to_record(saved)
    }

    async fn delete(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        ConfigRepository
            .delete_in_transaction(&self.transaction, tenant_id, id)
            .await
    }

    async fn record_namespace_change(&self, tenant_id: &str, namespace: &str) -> AppResult<i64> {
        self.authorization_cache
            .record_namespace_version_in_transaction(&self.transaction, tenant_id, namespace)
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
impl PersistenceTransaction for DatabaseConfigTransaction {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => self.transaction.commit_audited().await,
            TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.transaction.rollback().await.db()
    }
}

fn to_database_filter(filter: ConfigFilter<'_>) -> DatabaseConfigFilter<'_> {
    DatabaseConfigFilter {
        name: filter.name,
        key: filter.key,
    }
}

fn to_record(model: config::Model) -> AppResult<ConfigRecord> {
    Ok(ConfigRecord {
        id: model.id,
        name: model.name,
        key: model.key,
        value: model.value,
        portable: model.portable,
        remark: model.remark,
        created_at: utc_time_from_chrono(model.created_at).db_context("读取参数配置创建时间")?,
        updated_at: model.updated_at,
    })
}

fn to_entity(tenant_id: &str, record: ConfigRecord) -> AppResult<config::Model> {
    let created_at = chrono::DateTime::from_timestamp(
        record.created_at.unix_timestamp(),
        record.created_at.nanosecond(),
    )
    .ok_or_else(|| AppError::Database("参数配置创建时间超出 UTC 表示范围".into()))?;
    Ok(config::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        name: record.name,
        key: record.key,
        value: record.value,
        portable: record.portable,
        remark: record.remark,
        del_flag: config::Model::DEL_FLAG_NORMAL.into(),
        created_at,
        updated_at: record.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use time::UtcOffset;

    use super::{config, to_entity, to_record};

    #[test]
    fn config_created_at_round_trips_through_the_db_adapter() {
        let entity = config::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            name: "界面配置".into(),
            key: "ui.theme".into(),
            value: "dark".into(),
            portable: true,
            remark: Some("默认主题".into()),
            del_flag: config::Model::DEL_FLAG_NORMAL.into(),
            created_at: DateTime::from_timestamp(-1, 123_456_700).unwrap(),
            updated_at: DateTime::from_timestamp(1, 456_000_000).unwrap(),
        };
        let mut record = to_record(entity.clone()).unwrap();
        assert_eq!(record.created_at.offset(), UtcOffset::UTC);
        assert_eq!(record.created_at.unix_timestamp(), -1);
        assert_eq!(record.created_at.nanosecond(), 123_456_700);
        assert_eq!(record.updated_at, entity.updated_at);

        record.created_at = record
            .created_at
            .to_offset(UtcOffset::from_hms(8, 0, 0).unwrap());
        assert_eq!(to_entity("tenant-a", record).unwrap(), entity);
    }
}
