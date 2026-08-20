use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, ServiceAccountLock, ServiceAccountRepository,
    entities::{dept, service_account},
};
use ryframe_kernel::AppError;
use sea_orm::{
    ColumnTrait, DatabaseTransaction, EntityTrait, QueryFilter, QuerySelect, TransactionTrait,
    sea_query::LockType,
};

use crate::{
    AuthorizationMirrorTransaction, PersistenceFuture, ServiceAccountRecord,
    ServiceAccountWritePort, ServiceAccountWriteTransaction,
};

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn ServiceAccountWritePort> {
    Arc::new(LegacyServiceAccountWrite { database })
}

struct LegacyServiceAccountWrite {
    database: ControlDatabaseCluster,
}

struct LegacyServiceAccountWriteTransaction {
    transaction: DatabaseTransaction,
}

impl ServiceAccountWritePort for LegacyServiceAccountWrite {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ServiceAccountWriteTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(
                Box::new(LegacyServiceAccountWriteTransaction { transaction })
                    as Box<dyn ServiceAccountWriteTransaction>,
            )
        })
    }
}

impl ServiceAccountWriteTransaction for LegacyServiceAccountWriteTransaction {
    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction {
        &self.transaction
    }

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            ServiceAccountRepository
                .lock_tenant_in_txn(&self.transaction, tenant_id, ServiceAccountLock::Update)
                .await
                .map(|_| ())
        })
    }

    fn account_code_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            service_account::Entity::find()
                .filter(service_account::Column::TenantId.eq(tenant_id))
                .filter(service_account::Column::Code.eq(code))
                .one(&self.transaction)
                .await
                .map(|account| account.is_some())
                .map_err(database_error)
        })
    }

    fn department_exists<'a>(
        &'a self,
        tenant_id: &'a str,
        dept_id: i64,
    ) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            dept::Entity::find_by_id(dept_id)
                .filter(dept::Column::TenantId.eq(tenant_id))
                .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Share)
                .one(&self.transaction)
                .await
                .map(|department| department.is_some())
                .map_err(database_error)
        })
    }

    fn lock_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
    ) -> PersistenceFuture<'a, Option<ServiceAccountRecord>> {
        Box::pin(async move {
            ServiceAccountRepository
                .find_by_id_in_txn(
                    &self.transaction,
                    tenant_id,
                    account_id,
                    ServiceAccountLock::Update,
                )
                .await
                .map(|account| account.map(account_record))
        })
    }

    fn insert_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> PersistenceFuture<'a, ServiceAccountRecord> {
        Box::pin(async move {
            ServiceAccountRepository
                .insert_in_txn(&self.transaction, tenant_id, account_model(account))
                .await
                .map(account_record)
        })
    }

    fn save_account<'a>(
        &'a self,
        tenant_id: &'a str,
        account: ServiceAccountRecord,
    ) -> PersistenceFuture<'a, ServiceAccountRecord> {
        Box::pin(async move {
            ServiceAccountRepository
                .update_in_txn(&self.transaction, tenant_id, account_model(account))
                .await
                .map(account_record)
        })
    }

    fn replace_roles<'a>(
        &'a self,
        tenant_id: &'a str,
        account_id: i64,
        role_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            ServiceAccountRepository
                .replace_roles_in_txn(&self.transaction, tenant_id, account_id, role_ids)
                .await
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { crate::commit_current_audit(self.transaction).await })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn account_record(account: service_account::Model) -> ServiceAccountRecord {
    ServiceAccountRecord {
        id: account.id,
        tenant_id: account.tenant_id,
        code: account.code,
        name: account.name,
        description: account.description,
        dept_id: account.dept_id,
        status: account.status,
        authorization_version: account.authorization_version,
        max_requests_per_minute: account.max_requests_per_minute,
        created_by: account.created_by,
        deleted: account.del_flag == service_account::Model::DEL_FLAG_DELETED,
        created_at: account.created_at,
        updated_at: account.updated_at,
    }
}

fn account_model(account: ServiceAccountRecord) -> service_account::Model {
    service_account::Model {
        id: account.id,
        tenant_id: account.tenant_id,
        code: account.code,
        name: account.name,
        description: account.description,
        dept_id: account.dept_id,
        status: account.status,
        authorization_version: account.authorization_version,
        max_requests_per_minute: account.max_requests_per_minute,
        created_by: account.created_by,
        del_flag: if account.deleted {
            service_account::Model::DEL_FLAG_DELETED
        } else {
            service_account::Model::DEL_FLAG_NORMAL
        }
        .to_owned(),
        created_at: account.created_at,
        updated_at: account.updated_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn account_mapping_preserves_deleted_state() {
        let now = Utc.with_ymd_and_hms(2026, 8, 21, 1, 2, 3).unwrap();
        let model = service_account::Model {
            id: 1,
            tenant_id: "tenant-a".into(),
            code: "billing".into(),
            name: "结算服务".into(),
            description: None,
            dept_id: None,
            status: service_account::Model::STATUS_DISABLED.into(),
            authorization_version: 2,
            max_requests_per_minute: 60,
            created_by: 3,
            del_flag: service_account::Model::DEL_FLAG_DELETED.into(),
            created_at: now,
            updated_at: now,
        };

        let record = account_record(model);
        assert!(record.deleted);
        assert_eq!(
            account_model(record).del_flag,
            service_account::Model::DEL_FLAG_DELETED
        );
    }
}
