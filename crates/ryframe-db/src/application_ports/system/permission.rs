use std::{collections::BTreeSet, sync::Arc};

use crate::{
    AutoFill, ControlDatabaseCluster, FillContext, PermissionRepository, ProductRepository,
    ReadConsistency, Repository, TenantConfigTransferRepository, entities::permission,
};
use async_trait::async_trait;
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    sea_query::LockType,
};

use ryframe_application::system::ProductService;
use ryframe_application::{
    AuthorizationCache, PersistenceTransaction, TransactionAuditMode,
    ports::system::{
        PermissionReadPort, PermissionRecord, PermissionWritePort, PermissionWriteTransaction,
    },
};

use super::super::transaction::DatabasePortTransaction;

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn PermissionReadPort> {
    Arc::new(DatabasePermissionRead { database })
}

pub fn write_port(
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
    product: Arc<ProductService>,
) -> Arc<dyn PermissionWritePort> {
    Arc::new(DatabasePermissionWrite {
        database,
        authorization_cache,
        product,
    })
}

struct DatabasePermissionRead {
    database: ControlDatabaseCluster,
}

struct DatabasePermissionWrite {
    database: ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
    product: Arc<ProductService>,
}

struct DatabasePermissionWriteTransaction {
    transaction: DatabasePortTransaction,
    authorization_cache: AuthorizationCache,
    product: Arc<ProductService>,
}

#[async_trait]
impl PermissionReadPort for DatabasePermissionRead {
    async fn find_role_codes(
        &self,
        tenant_id: &str,
        role_ids: &[i64],
    ) -> ryframe_kernel::AppResult<Vec<String>> {
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
    }

    async fn find_role_ids(
        &self,
        tenant_id: &str,
        role_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<i64>> {
        let database = self.strong_read();
        PermissionRepository
            .find_role_perm_ids(&database, tenant_id, role_id)
            .await
    }

    async fn find_all(&self, tenant_id: &str) -> ryframe_kernel::AppResult<Vec<PermissionRecord>> {
        let database = self.strong_read();
        PermissionRepository
            .find_all(&database, tenant_id)
            .await
            .map(|permissions| permissions.into_iter().map(to_record).collect())
    }

    async fn find_by_id(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<PermissionRecord>> {
        let database = self.strong_read();
        Ok(PermissionRepository
            .find_by_id(&database, tenant_id, id)
            .await?
            .map(to_record))
    }
}

impl DatabasePermissionRead {
    fn strong_read(&self) -> sea_orm::DatabaseConnection {
        self.database
            .select_read(ReadConsistency::Strong)
            .connection
    }
}

#[async_trait]
impl PermissionWritePort for DatabasePermissionWrite {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn PermissionWriteTransaction>> {
        let transaction = self
            .database
            .write()
            .begin()
            .await
            .map_err(database_error)?;
        Ok(Box::new(DatabasePermissionWriteTransaction {
            transaction: transaction.into(),
            authorization_cache: self.authorization_cache.clone(),
            product: Arc::clone(&self.product),
        }) as Box<dyn PermissionWriteTransaction>)
    }
}

#[async_trait]
impl PermissionWriteTransaction for DatabasePermissionWriteTransaction {
    async fn lock_configuration(&self, tenant_id: &str) -> ryframe_kernel::AppResult<()> {
        TenantConfigTransferRepository
            .lock_tenant_configuration_in_txn(&self.transaction, tenant_id, None)
            .await
            .map(|_| ())
    }

    async fn find_by_id_for_update(
        &self,
        tenant_id: &str,
        id: i64,
    ) -> ryframe_kernel::AppResult<Option<PermissionRecord>> {
        Ok(PermissionRepository
            .find_by_id_for_update(&self.transaction, tenant_id, id)
            .await?
            .map(to_record))
    }

    async fn find_by_code_for_update(
        &self,
        tenant_id: &str,
        code: &str,
    ) -> ryframe_kernel::AppResult<Option<PermissionRecord>> {
        Ok(permission::Entity::find()
            .filter(permission::Column::TenantId.eq(tenant_id))
            .filter(permission::Column::Code.eq(code))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .map_err(database_error)?
            .map(to_record))
    }

    async fn find_all_for_update(
        &self,
        tenant_id: &str,
    ) -> ryframe_kernel::AppResult<Vec<PermissionRecord>> {
        permission::Entity::find()
            .filter(permission::Column::TenantId.eq(tenant_id))
            .order_by_asc(permission::Column::Id)
            .lock(LockType::Update)
            .all(&self.transaction)
            .await
            .map(|permissions| permissions.into_iter().map(to_record).collect())
            .map_err(database_error)
    }

    async fn insert(
        &self,
        tenant_id: &str,
        record: PermissionRecord,
    ) -> ryframe_kernel::AppResult<PermissionRecord> {
        let mut entity = to_entity(tenant_id, record);
        entity.fill_on_insert(&FillContext::new())?;
        PermissionRepository
            .insert_in_transaction(&self.transaction, tenant_id, entity)
            .await
            .map(to_record)
    }

    async fn update(
        &self,
        tenant_id: &str,
        record: PermissionRecord,
    ) -> ryframe_kernel::AppResult<PermissionRecord> {
        let mut entity = to_entity(tenant_id, record);
        entity.fill_on_update(&FillContext::new())?;
        PermissionRepository
            .update_in_transaction(&self.transaction, tenant_id, entity)
            .await
            .map(to_record)
    }

    async fn is_referenced(&self, tenant_id: &str, id: i64) -> ryframe_kernel::AppResult<bool> {
        PermissionRepository
            .is_referenced(&self.transaction, tenant_id, id)
            .await
    }

    async fn delete(&self, tenant_id: &str, id: i64) -> ryframe_kernel::AppResult<()> {
        PermissionRepository
            .delete_in_transaction(&self.transaction, tenant_id, id)
            .await
    }

    async fn filter_syncable_codes(
        &self,
        tenant_id: &str,
        codes: BTreeSet<String>,
    ) -> ryframe_kernel::AppResult<BTreeSet<String>> {
        let snapshot = ProductRepository
            .tenant_product(&self.transaction, tenant_id)
            .await?
            .map(super::super::product::tenant_snapshot)
            .ok_or_else(|| ryframe_kernel::AppError::NotFound("租户不存在".into()))?;
        self.product
            .filter_syncable_permission_codes(snapshot, codes)
    }

    async fn increment_authorization_epoch(
        &self,
        tenant_id: &str,
    ) -> ryframe_kernel::AppResult<i32> {
        self.authorization_cache
            .increment_tenant_epoch_in_transaction(&self.transaction, tenant_id)
            .await
    }

    async fn increment_configuration_version(
        &self,
        tenant_id: &str,
    ) -> ryframe_kernel::AppResult<()> {
        TenantConfigTransferRepository
            .increment_configuration_version_in_txn(&self.transaction, tenant_id)
            .await
            .map(|_| ())
    }
}

#[async_trait]
impl PersistenceTransaction for DatabasePermissionWriteTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => self.transaction.commit_audited().await,
            TransactionAuditMode::Skip => self.transaction.commit().await.map_err(database_error),
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.map_err(database_error)
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
