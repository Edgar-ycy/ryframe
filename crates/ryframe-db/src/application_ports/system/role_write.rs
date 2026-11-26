use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    PermissionRepository, ProductRepository, RoleRepository, TenantConfigTransferRepository,
    TenantRepository,
    entities::{dept, permission, role},
};
use async_trait::async_trait;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait, sea_query::LockType,
};
use time::UtcOffset;

use ryframe_application::system::platform::ProductService;
use ryframe_application::{
    AuthorizationCache, PersistenceTransaction, TransactionAuditMode,
    ports::system::{RolePermissionRef, RoleRecord, RoleWritePort, RoleWriteTransaction},
};

use super::super::transaction::DatabasePortTransaction;
use super::role_read::to_record;

pub fn port(
    database: crate::ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
    product: Arc<ProductService>,
) -> Arc<dyn RoleWritePort> {
    Arc::new(DatabaseRoleWrite {
        database,
        authorization_cache,
        product,
    })
}

struct DatabaseRoleWrite {
    database: crate::ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
    product: Arc<ProductService>,
}

struct DatabaseRoleWriteTransaction {
    transaction: DatabasePortTransaction,
    authorization_cache: AuthorizationCache,
    product: Arc<ProductService>,
}

#[async_trait]
impl RoleWritePort for DatabaseRoleWrite {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn RoleWriteTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseRoleWriteTransaction {
            transaction: transaction.into(),
            authorization_cache: self.authorization_cache.clone(),
            product: Arc::clone(&self.product),
        }) as Box<dyn RoleWriteTransaction>)
    }
}

#[async_trait]
impl RoleWriteTransaction for DatabaseRoleWriteTransaction {
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
    ) -> ryframe_kernel::AppResult<Option<RoleRecord>> {
        RoleRepository
            .find_by_id_for_update(&self.transaction, tenant_id, id)
            .await?
            .map(to_record)
            .transpose()
    }

    async fn find_by_code_for_update(
        &self,
        tenant_id: &str,
        code: &str,
    ) -> ryframe_kernel::AppResult<Option<RoleRecord>> {
        role::Entity::find()
            .filter(role::Column::TenantId.eq(tenant_id))
            .filter(role::Column::Code.eq(code))
            .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
            .lock(LockType::Update)
            .one(&self.transaction)
            .await
            .db()?
            .map(to_record)
            .transpose()
    }

    async fn count_available_super_roles(
        &self,
        tenant_id: &str,
    ) -> ryframe_kernel::AppResult<usize> {
        RoleRepository
            .count_available_super_roles_for_update(&self.transaction, tenant_id)
            .await
    }

    async fn ensure_role_quota(&self, tenant_id: &str) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .ensure_role_quota_in_txn(&self.transaction, tenant_id)
            .await
    }

    async fn insert(
        &self,
        tenant_id: &str,
        record: RoleRecord,
    ) -> ryframe_kernel::AppResult<RoleRecord> {
        let entity = to_entity(tenant_id, record)?;
        if entity.tenant_id != tenant_id {
            return Err(ryframe_kernel::AppError::Authorization(
                "角色租户不匹配".into(),
            ));
        }
        role::ActiveModel::from(entity)
            .insert(&self.transaction)
            .await
            .db()
            .and_then(to_record)
    }

    async fn update(
        &self,
        tenant_id: &str,
        record: RoleRecord,
    ) -> ryframe_kernel::AppResult<RoleRecord> {
        let entity = to_entity(tenant_id, record)?;
        role::ActiveModel::from(entity)
            .reset_all()
            .update(&self.transaction)
            .await
            .db()
            .and_then(to_record)
    }

    async fn delete_many(&self, tenant_id: &str, ids: &[i64]) -> ryframe_kernel::AppResult<u64> {
        RoleRepository
            .delete_many(&self.transaction, tenant_id, ids)
            .await
    }

    async fn find_permissions_for_update(
        &self,
        tenant_id: &str,
        permission_ids: &[i64],
    ) -> ryframe_kernel::AppResult<Vec<RolePermissionRef>> {
        permission::Entity::find()
            .filter(permission::Column::TenantId.eq(tenant_id))
            .filter(permission::Column::Id.is_in(permission_ids.iter().copied()))
            .order_by_asc(permission::Column::Id)
            .lock(LockType::Update)
            .all(&self.transaction)
            .await
            .map(|permissions| {
                permissions
                    .into_iter()
                    .map(|permission| RolePermissionRef {
                        id: permission.id,
                        code: permission.code,
                    })
                    .collect()
            })
            .db()
    }

    async fn ensure_permission_codes_enabled(
        &self,
        tenant_id: &str,
        permission_codes: &[String],
    ) -> ryframe_kernel::AppResult<()> {
        let snapshot = ProductRepository
            .tenant_product(&self.transaction, tenant_id)
            .await?
            .map(super::super::product::tenant_snapshot)
            .ok_or_else(|| ryframe_kernel::AppError::NotFound("租户不存在".into()))?;
        self.product
            .ensure_permission_codes_enabled(snapshot, permission_codes)
    }

    async fn assign_permissions(
        &self,
        tenant_id: &str,
        role_id: i64,
        permission_ids: &[i64],
    ) -> ryframe_kernel::AppResult<()> {
        PermissionRepository
            .assign_perms(&self.transaction, tenant_id, role_id, permission_ids)
            .await
    }

    async fn find_departments_for_update(
        &self,
        tenant_id: &str,
        department_ids: &[i64],
    ) -> ryframe_kernel::AppResult<Vec<i64>> {
        dept::Entity::find()
            .filter(dept::Column::TenantId.eq(tenant_id))
            .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
            .filter(dept::Column::Id.is_in(department_ids.iter().copied()))
            .order_by_asc(dept::Column::Id)
            .lock(LockType::Update)
            .all(&self.transaction)
            .await
            .map(|departments| departments.into_iter().map(|dept| dept.id).collect())
            .db()
    }

    async fn replace_data_scope(
        &self,
        tenant_id: &str,
        role_id: i64,
        data_scope: &str,
        department_ids: &[i64],
    ) -> ryframe_kernel::AppResult<()> {
        RoleRepository
            .replace_data_scope(
                &self.transaction,
                tenant_id,
                role_id,
                data_scope,
                department_ids,
            )
            .await
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
impl PersistenceTransaction for DatabaseRoleWriteTransaction {
    async fn commit(
        self: Box<Self>,
        audit_mode: TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        match audit_mode {
            TransactionAuditMode::CurrentRequest => self.transaction.commit_audited().await,
            TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.db()
    }
}

fn to_entity(tenant_id: &str, record: RoleRecord) -> ryframe_kernel::AppResult<role::Model> {
    let created_at = record
        .created_at
        .checked_to_offset(UtcOffset::UTC)
        .ok_or_else(|| {
            ryframe_kernel::AppError::Database("角色创建时间超出 UTC 表示范围".into())
        })?;
    Ok(role::Model {
        id: record.id,
        tenant_id: tenant_id.to_owned(),
        name: record.name,
        code: record.code,
        is_super: record.is_super,
        data_scope: record.data_scope,
        status: record.status,
        sort: record.sort,
        remark: record.remark,
        del_flag: role::Model::DEL_FLAG_NORMAL.into(),
        created_at,
        updated_at: record.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use time::{OffsetDateTime, UtcOffset, macros::datetime};

    use super::{role, to_entity, to_record};

    #[test]
    fn role_created_at_round_trips_through_the_db_adapter() {
        let updated_at = datetime!(1970-01-01 00:00:01.456 UTC);
        let offset = UtcOffset::from_hms(8, 0, 0).unwrap();
        for (seconds, nanos) in [(0, 0), (-1, 123_456_700)] {
            let created_at = OffsetDateTime::from_unix_timestamp(seconds)
                .unwrap()
                .replace_nanosecond(nanos)
                .unwrap();
            let entity = role::Model {
                id: 1,
                tenant_id: "tenant-a".into(),
                name: "工单审核员".into(),
                code: "reviewer".into(),
                is_super: 0,
                data_scope: "1".into(),
                status: "1".into(),
                sort: 0,
                remark: None,
                del_flag: role::Model::DEL_FLAG_NORMAL.into(),
                created_at,
                updated_at,
            };
            let mut record = to_record(entity).unwrap();
            assert_eq!(record.created_at.offset(), UtcOffset::UTC);
            assert_eq!(record.created_at.unix_timestamp(), seconds);
            assert_eq!(record.created_at.nanosecond(), nanos);
            assert_eq!(record.updated_at, updated_at);

            record.created_at = record.created_at.to_offset(offset);
            let restored = to_entity("tenant-a", record).unwrap();
            assert_eq!(restored.created_at, created_at);
            assert_eq!(restored.updated_at, updated_at);
        }
    }
}
