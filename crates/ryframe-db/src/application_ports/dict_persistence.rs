use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, DictDataRepository, DictTypeFilter as DatabaseDictTypeFilter,
    DictTypeRepository, ReadConsistency, TenantConfigTransferRepository,
    entities::{dict_data, dict_type},
};
use ryframe_kernel::{ExportCursorWindow, PageResult, ValidatedPageQuery};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait, sea_query::LockType,
};

use ryframe_application::{
    ControlTransaction, DictDataRecord, DictPersistencePort, DictTransaction, DictTypeFilter,
    DictTypeRecord, PersistenceFuture,
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

impl DictPersistencePort for DatabaseDictPersistence {
    fn find_types_by_page<'a>(
        &'a self,
        tenant_id: &'a str,
        page: ValidatedPageQuery,
        filter: DictTypeFilter<'a>,
    ) -> PersistenceFuture<'a, PageResult<DictTypeRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find_type_export_batch<'a>(
        &'a self,
        tenant_id: &'a str,
        filter: DictTypeFilter<'a>,
        window: ExportCursorWindow,
    ) -> PersistenceFuture<'a, Vec<DictTypeRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            DictTypeRepository
                .find_for_export_after_id(&database, tenant_id, &to_database_filter(filter), window)
                .await
                .map(|records| records.into_iter().map(to_type_record).collect())
        })
    }

    fn find_data_by_type<'a>(
        &'a self,
        tenant_id: &'a str,
        type_code: &'a str,
    ) -> PersistenceFuture<'a, Vec<DictDataRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            DictDataRepository
                .find_by_type_code(&database, tenant_id, type_code)
                .await
                .map(|records| records.into_iter().map(to_data_record).collect())
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn DictTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseDictTransaction { transaction }) as Box<dyn DictTransaction>)
        })
    }
}

impl DictTransaction for DatabaseDictTransaction {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
                .await
                .map(|_| ())
        })
    }

    fn find_type_by_code_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, Option<DictTypeRecord>> {
        Box::pin(async move {
            Ok(dict_type::Entity::find()
                .filter(dict_type::Column::TenantId.eq(tenant_id))
                .filter(dict_type::Column::Code.eq(code))
                .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_type_record))
        })
    }

    fn find_type_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<DictTypeRecord>> {
        Box::pin(async move {
            Ok(dict_type::Entity::find_by_id(id)
                .filter(dict_type::Column::TenantId.eq(tenant_id))
                .filter(dict_type::Column::DelFlag.eq(dict_type::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_type_record))
        })
    }

    fn insert_type<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DictTypeRecord,
    ) -> PersistenceFuture<'a, DictTypeRecord> {
        Box::pin(async move {
            DictTypeRepository
                .insert_in_transaction(
                    &self.transaction,
                    tenant_id,
                    to_type_entity(tenant_id, record),
                )
                .await
                .map(to_type_record)
        })
    }

    fn update_type<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DictTypeRecord,
    ) -> PersistenceFuture<'a, DictTypeRecord> {
        Box::pin(async move {
            DictTypeRepository
                .update_in_transaction(
                    &self.transaction,
                    tenant_id,
                    to_type_entity(tenant_id, record),
                )
                .await
                .map(to_type_record)
        })
    }

    fn delete_type<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            DictTypeRepository
                .delete_in_transaction(&self.transaction, tenant_id, id)
                .await
        })
    }

    fn find_data_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<DictDataRecord>> {
        Box::pin(async move {
            Ok(dict_data::Entity::find_by_id(id)
                .filter(dict_data::Column::TenantId.eq(tenant_id))
                .filter(dict_data::Column::DelFlag.eq(dict_data::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_data_record))
        })
    }

    fn insert_data<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DictDataRecord,
    ) -> PersistenceFuture<'a, DictDataRecord> {
        Box::pin(async move {
            DictDataRepository
                .insert_in_transaction(
                    &self.transaction,
                    tenant_id,
                    to_data_entity(tenant_id, record),
                )
                .await
                .map(to_data_record)
        })
    }

    fn update_data<'a>(
        &'a self,
        tenant_id: &'a str,
        record: DictDataRecord,
    ) -> PersistenceFuture<'a, DictDataRecord> {
        Box::pin(async move {
            DictDataRepository
                .update_in_transaction(
                    &self.transaction,
                    tenant_id,
                    to_data_entity(tenant_id, record),
                )
                .await
                .map(to_data_record)
        })
    }

    fn delete_data<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            DictDataRepository
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

impl ControlTransaction for DatabaseDictTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(
            async move { super::audit_persistence::commit_current_audit(self.transaction).await },
        )
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

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
