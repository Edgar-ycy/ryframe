use std::{collections::BTreeSet, sync::Arc};

use ryframe_db::{
    AutoFill, ControlDatabaseCluster, FillContext, PermissionRepository, ReadConsistency,
    Repository, TenantConfigTransferRepository, entities::permission,
};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    sea_query::LockType,
};

use crate::system::ProductService;
use crate::{
    AuthorizationCache, ControlTransaction, PermissionReadPort, PermissionRecord,
    PermissionWritePort, PermissionWriteTransaction, PersistenceFuture,
};

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn PermissionReadPort> {
    Arc::new(LegacyPermissionRead { database })
}

pub fn write_port(
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
    product_service: Arc<ProductService>,
) -> Arc<dyn PermissionWritePort> {
    Arc::new(LegacyPermissionWrite {
        database,
        authorization_cache,
        product_service,
    })
}

struct LegacyPermissionRead {
    database: ControlDatabaseCluster,
}

struct LegacyPermissionWrite {
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
    product_service: Arc<ProductService>,
}

struct LegacyPermissionWriteTransaction {
    transaction: sea_orm::DatabaseTransaction,
    authorization_cache: AuthorizationCache,
    product_service: Arc<ProductService>,
}

impl PermissionReadPort for LegacyPermissionRead {
    fn find_role_codes<'a>(
        &'a self,
        tenant_id: &'a str,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<String>> {
        Box::pin(async move {
            let database = self.strong_read();
            PermissionRepository
                .find_role_perms(&database, tenant_id, role_ids)
                .await
                .map(|permissions| {
                    permissions
                        .into_iter()
                        .map(|permission| permission.code)
                        .collect()
                })
        })
    }

    fn find_role_ids<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
    ) -> PersistenceFuture<'a, Vec<i64>> {
        Box::pin(async move {
            let database = self.strong_read();
            PermissionRepository
                .find_role_perm_ids(&database, tenant_id, role_id)
                .await
        })
    }

    fn find_all<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, Vec<PermissionRecord>> {
        Box::pin(async move {
            let database = self.strong_read();
            PermissionRepository
                .find_all(&database, tenant_id)
                .await
                .map(|permissions| permissions.into_iter().map(to_record).collect())
        })
    }

    fn find_by_id<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<PermissionRecord>> {
        Box::pin(async move {
            let database = self.strong_read();
            Ok(PermissionRepository
                .find_by_id(&database, tenant_id, id)
                .await?
                .map(to_record))
        })
    }
}

impl LegacyPermissionRead {
    fn strong_read(&self) -> sea_orm::DatabaseConnection {
        self.database
            .select_read(ReadConsistency::Strong)
            .connection
    }
}

impl PermissionWritePort for LegacyPermissionWrite {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn PermissionWriteTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(LegacyPermissionWriteTransaction {
                transaction,
                authorization_cache: self.authorization_cache.clone(),
                product_service: Arc::clone(&self.product_service),
            }) as Box<dyn PermissionWriteTransaction>)
        })
    }
}

impl PermissionWriteTransaction for LegacyPermissionWriteTransaction {
    fn lock_configuration<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantConfigTransferRepository
                .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
                .await
                .map(|_| ())
        })
    }

    fn find_by_id_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        id: i64,
    ) -> PersistenceFuture<'a, Option<PermissionRecord>> {
        Box::pin(async move {
            Ok(PermissionRepository
                .find_by_id_for_update(&self.transaction, tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn find_by_code_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, Option<PermissionRecord>> {
        Box::pin(async move {
            Ok(permission::Entity::find()
                .filter(permission::Column::TenantId.eq(tenant_id))
                .filter(permission::Column::Code.eq(code))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_record))
        })
    }

    fn find_all_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Vec<PermissionRecord>> {
        Box::pin(async move {
            permission::Entity::find()
                .filter(permission::Column::TenantId.eq(tenant_id))
                .order_by_asc(permission::Column::Id)
                .lock(LockType::Update)
                .all(&self.transaction)
                .await
                .map(|permissions| permissions.into_iter().map(to_record).collect())
                .map_err(database_error)
        })
    }

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: PermissionRecord,
    ) -> PersistenceFuture<'a, PermissionRecord> {
        Box::pin(async move {
            let mut entity = to_entity(tenant_id, record);
            entity.fill_on_insert(&FillContext::new())?;
            PermissionRepository
                .insert_in_transaction(&self.transaction, tenant_id, entity)
                .await
                .map(to_record)
        })
    }

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: PermissionRecord,
    ) -> PersistenceFuture<'a, PermissionRecord> {
        Box::pin(async move {
            let mut entity = to_entity(tenant_id, record);
            entity.fill_on_update(&FillContext::new())?;
            PermissionRepository
                .update_in_transaction(&self.transaction, tenant_id, entity)
                .await
                .map(to_record)
        })
    }

    fn is_referenced<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            PermissionRepository
                .is_referenced(&self.transaction, tenant_id, id)
                .await
        })
    }

    fn delete<'a>(&'a self, tenant_id: &'a str, id: i64) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            PermissionRepository
                .delete_in_transaction(&self.transaction, tenant_id, id)
                .await
        })
    }

    fn filter_syncable_codes<'a>(
        &'a self,
        tenant_id: &'a str,
        codes: BTreeSet<String>,
    ) -> PersistenceFuture<'a, BTreeSet<String>> {
        Box::pin(async move {
            self.product_service
                .filter_syncable_permission_codes_in_txn(&self.transaction, tenant_id, codes)
                .await
        })
    }

    fn increment_authorization_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, i32> {
        Box::pin(async move {
            self.authorization_cache
                .increment_tenant_epoch_in_transaction(&self.transaction, tenant_id)
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

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

impl ControlTransaction for LegacyPermissionWriteTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { crate::commit_current_audit(self.transaction).await })
    }
}

fn to_record(model: permission::Model) -> PermissionRecord {
    PermissionRecord {
        id: model.id,
        name: model.name,
        code: model.code,
        parent_id: model.parent_id,
        perm_type: model.perm_type,
        icon: model.icon,
        sort: model.sort,
        status: model.status,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_entity(tenant_id: &str, record: PermissionRecord) -> permission::Model {
    permission::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        name: record.name,
        code: record.code,
        parent_id: record.parent_id,
        perm_type: record.perm_type,
        icon: record.icon,
        sort: record.sort,
        status: record.status,
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
