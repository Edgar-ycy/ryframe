use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, DictDataRepository, DictTypeFilter as DatabaseDictTypeFilter,
    DictTypeRepository, ReadConsistency, TenantConfigTransferRepository,
    entities::{dict_data, dict_type},
};
use async_trait::async_trait;
use ryframe_kernel::{AppResult, ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    PersistenceTransaction, TransactionAuditMode,
    ports::system::{
        DictDataRecord, DictPersistencePort, DictTransaction, DictTypeFilter, DictTypeRecord,
    },
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn DictPersistencePort> {
    Arc::new(DatabaseDictPersistence { database })
}

struct DatabaseDictPersistence {
    database: ControlDatabaseCluster,
}

struct DatabaseDictTransaction {
    transaction: sea_orm::DatabaseTransaction,
}

#[async_trait]
impl DictPersistencePort for DatabaseDictPersistence {
    async fn find_types_by_page(
        &self,
        tenant_id: &str,
        page: ValidatedPageQuery,
        filter: DictTypeFilter<'_>,
    ) -> AppResult<PageResult<DictTypeRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        let filter = to_database_filter(filter);
        let result = DictTypeRepository
            .find_by_page_filtered(&database, tenant_id, &page, &filter)
            .await?;
        Ok(PageResult::new(
            result.records.into_iter().map(to_type_record).collect(),
            result.total,
            &page,
        ))
    }

    async fn find_type_export_batch(
        &self,
        tenant_id: &str,
        filter: DictTypeFilter<'_>,
        window: ExportCursorWindow,
    ) -> AppResult<Vec<DictTypeRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        DictTypeRepository
            .find_for_export_after_id(&database, tenant_id, &to_database_filter(filter), window)
            .await
            .map(|records| records.into_iter().map(to_type_record).collect())
    }

    async fn find_data_by_type(
        &self,
        tenant_id: &str,
        type_code: &str,
    ) -> AppResult<Vec<DictDataRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        DictDataRepository
            .find_by_type_code(&database, tenant_id, type_code)
            .await
            .map(|records| records.into_iter().map(to_data_record).collect())
    }

    async fn begin(&self) -> AppResult<Box<dyn DictTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseDictTransaction { transaction }) as Box<dyn DictTransaction>)
    }
}

#[async_trait]
impl DictTransaction for DatabaseDictTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> AppResult<()> {
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
            .await
            .map(|_| ())
    }

    async fn find_type_by_code_for_update(
        &self,
        tenant_id: &str,
        code: &str,
    ) -> AppResult<Option<DictTypeRecord>> {
        Ok(dict_type::Entity::find()
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::Code.eq(code))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()?
            .map(to_type_record))
    }

    async fn find_type_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<DictTypeRecord>> {
        Ok(dict_type::Entity::find_by_id(id)
            .filter(dict_type::Column::TenantId.eq(tenant_id))
            .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()?
            .map(to_type_record))
    }

    async fn insert_type(
        &self,
        tenant_id: &str,
        record: DictTypeRecord,
    ) -> AppResult<DictTypeRecord> {
        DictTypeRepository
            .insert_in_transaction(
                &self.transaction,
                tenant_id,
                to_type_entity(tenant_id, record),
            )
            .await
            .map(to_type_record)
    }

    async fn update_type(
        &self,
        tenant_id: &str,
        record: DictTypeRecord,
    ) -> AppResult<DictTypeRecord> {
        DictTypeRepository
            .update_in_transaction(
                &self.transaction,
                tenant_id,
                to_type_entity(tenant_id, record),
            )
            .await
            .map(to_type_record)
    }

    async fn delete_type(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        DictTypeRepository
            .delete_in_transaction(&self.transaction, tenant_id, id)
            .await
    }

    async fn find_data_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> AppResult<Option<DictDataRecord>> {
        Ok(dict_data::Entity::find_by_id(id)
            .filter(dict_data::Column::TenantId.eq(tenant_id))
            .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()?
            .map(to_data_record))
    }

    async fn insert_data(
        &self,
        tenant_id: &str,
        record: DictDataRecord,
    ) -> AppResult<DictDataRecord> {
        DictDataRepository
            .insert_in_transaction(
                &self.transaction,
                tenant_id,
                to_data_entity(tenant_id, record),
            )
            .await
            .map(to_data_record)
    }

    async fn update_data(
        &self,
        tenant_id: &str,
        record: DictDataRecord,
    ) -> AppResult<DictDataRecord> {
        DictDataRepository
            .update_in_transaction(
                &self.transaction,
                tenant_id,
                to_data_entity(tenant_id, record),
            )
            .await
            .map(to_data_record)
    }

    async fn delete_data(&self, tenant_id: &str, id: i64) -> AppResult<()> {
        DictDataRepository
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
impl PersistenceTransaction for DatabaseDictTransaction {
    async fn commit(self: Box<Self>, audit_mode: TransactionAuditMode) -> AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => {
                super::super::audit::commit_current_audit(self.transaction).await
            }
            TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> AppResult<()> {
        self.transaction.rollback().await.db()
    }
}

fn to_database_filter(filter: DictTypeFilter<'_>) -> DatabaseDictTypeFilter<'_> {
    DatabaseDictTypeFilter {
        name: filter.name,
        code: filter.code,
        status: filter.status,
    }
}

fn to_type_record(model: dict_type::Model) -> DictTypeRecord {
    DictTypeRecord {
        id: model.id,
        name: model.name,
        code: model.code,
        status: model.status,
        remark: model.remark,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_type_entity(tenant_id: &str, record: DictTypeRecord) -> dict_type::Model {
    dict_type::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        name: record.name,
        code: record.code,
        status: record.status,
        remark: record.remark,
        del_flag: dict_type::Model::DEL_FLAG_NORMAL.into(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn to_data_record(model: dict_data::Model) -> DictDataRecord {
    DictDataRecord {
        id: model.id,
        type_code: model.type_code,
        label: model.label,
        value: model.value,
        sort: model.sort,
        status: model.status,
        css_class: model.css_class,
        remark: model.remark,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_data_entity(tenant_id: &str, record: DictDataRecord) -> dict_data::Model {
    dict_data::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        type_code: record.type_code,
        label: record.label,
        value: record.value,
        sort: record.sort,
        status: record.status,
        css_class: record.css_class,
        remark: record.remark,
        del_flag: dict_data::Model::DEL_FLAG_NORMAL.into(),
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}
