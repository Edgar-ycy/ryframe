use std::sync::Arc;

use crate::{
    PermissionRepository, ProductRepository, RoleRepository, TenantConfigTransferRepository,
    TenantRepository,
    entities::{dept, permission, role},
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    TransactionTrait, sea_query::LockType,
};

use ryframe_application::system::ProductService;
use ryframe_application::{
    AuthorizationCache, ControlTransaction, PersistenceFuture,
    ports::system::{RolePermissionRef, RoleRecord, RoleWritePort, RoleWriteTransaction},
};

use super::super::control_transaction::DatabasePortTransaction;

pub fn port(
    database: crate::ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
    product_service: Arc<ProductService>,
) -> Arc<dyn RoleWritePort> {
    Arc::new(DatabaseRoleWrite {
        database,
        authorization_cache,
        product_service,
    })
}

struct DatabaseRoleWrite {
    database: crate::ControlDatabaseCluster,
    authorization_cache: AuthorizationCache,
    product_service: Arc<ProductService>,
}

struct DatabaseRoleWriteTransaction {
    transaction: DatabasePortTransaction,
    authorization_cache: AuthorizationCache,
    product_service: Arc<ProductService>,
}

impl RoleWritePort for DatabaseRoleWrite {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn RoleWriteTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(DatabaseRoleWriteTransaction {
                transaction: transaction.into(),
                authorization_cache: self.authorization_cache.clone(),
                product_service: Arc::clone(&self.product_service),
            }) as Box<dyn RoleWriteTransaction>)
        })
    }
}

impl RoleWriteTransaction for DatabaseRoleWriteTransaction {
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
    ) -> PersistenceFuture<'a, Option<RoleRecord>> {
        Box::pin(async move {
            Ok(RoleRepository
                .find_by_id_for_update(&self.transaction, tenant_id, id)
                .await?
                .map(to_record))
        })
    }

    fn find_by_code_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        code: &'a str,
    ) -> PersistenceFuture<'a, Option<RoleRecord>> {
        Box::pin(async move {
            Ok(role::Entity::find()
                .filter(role::Column::TenantId.eq(tenant_id))
                .filter(role::Column::Code.eq(code))
                .filter(role::Column::DelFlag.eq(role::Model::DEL_FLAG_NORMAL))
                .lock(LockType::Update)
                .one(&self.transaction)
                .await
                .map_err(database_error)?
                .map(to_record))
        })
    }

    fn count_available_super_roles<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, usize> {
        Box::pin(async move {
            RoleRepository
                .count_available_super_roles_for_update(&self.transaction, tenant_id)
                .await
        })
    }

    fn ensure_role_quota<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantRepository
                .ensure_role_quota_in_txn(&self.transaction, tenant_id)
                .await
        })
    }

    fn insert<'a>(
        &'a self,
        tenant_id: &'a str,
        record: RoleRecord,
    ) -> PersistenceFuture<'a, RoleRecord> {
        Box::pin(async move {
            let entity = to_entity(tenant_id, record);
            if entity.tenant_id != tenant_id {
                return Err(ryframe_kernel::AppError::Authorization(
                    "角色租户不匹配".into(),
                ));
            }
            role::ActiveModel::from(entity)
                .insert(&self.transaction)
                .await
                .map(to_record)
                .map_err(database_error)
        })
    }

    fn update<'a>(
        &'a self,
        tenant_id: &'a str,
        record: RoleRecord,
    ) -> PersistenceFuture<'a, RoleRecord> {
        Box::pin(async move {
            let entity = to_entity(tenant_id, record);
            role::ActiveModel::from(entity)
                .reset_all()
                .update(&self.transaction)
                .await
                .map(to_record)
                .map_err(database_error)
        })
    }

    fn delete_many<'a>(&'a self, tenant_id: &'a str, ids: &'a [i64]) -> PersistenceFuture<'a, u64> {
        Box::pin(async move {
            RoleRepository
                .delete_many(&self.transaction, tenant_id, ids)
                .await
        })
    }

    fn find_permissions_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<RolePermissionRef>> {
        Box::pin(async move {
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
                .map_err(database_error)
        })
    }

    fn ensure_permission_codes_enabled<'a>(
        &'a self,
        tenant_id: &'a str,
        permission_codes: &'a [String],
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            let snapshot = ProductRepository
                .tenant_product(&self.transaction, tenant_id)
                .await?
                .map(super::super::product::tenant_snapshot)
                .ok_or_else(|| ryframe_kernel::AppError::NotFound("租户不存在".into()))?;
            self.product_service
                .ensure_permission_codes_enabled(snapshot, permission_codes)
        })
    }

    fn assign_permissions<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
        permission_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            PermissionRepository
                .assign_perms(&self.transaction, tenant_id, role_id, permission_ids)
                .await
        })
    }

    fn find_departments_for_update<'a>(
        &'a self,
        tenant_id: &'a str,
        department_ids: &'a [i64],
    ) -> PersistenceFuture<'a, Vec<i64>> {
        Box::pin(async move {
            dept::Entity::find()
                .filter(dept::Column::TenantId.eq(tenant_id))
                .filter(dept::Column::DelFlag.eq(dept::Model::DEL_FLAG_NORMAL))
                .filter(dept::Column::Id.is_in(department_ids.iter().copied()))
                .order_by_asc(dept::Column::Id)
                .lock(LockType::Update)
                .all(&self.transaction)
                .await
                .map(|departments| departments.into_iter().map(|dept| dept.id).collect())
                .map_err(database_error)
        })
    }

    fn replace_data_scope<'a>(
        &'a self,
        tenant_id: &'a str,
        role_id: i64,
        data_scope: &'a str,
        department_ids: &'a [i64],
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            RoleRepository
                .replace_data_scope(
                    &self.transaction,
                    tenant_id,
                    role_id,
                    data_scope,
                    department_ids,
                )
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
}

impl ControlTransaction for DatabaseRoleWriteTransaction {
    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move {
            super::super::audit_persistence::commit_current_audit(self.transaction.into_inner())
                .await
        })
    }
}

fn to_record(model: role::Model) -> RoleRecord {
    RoleRecord {
        id: model.id,
        name: model.name,
        code: model.code,
        is_super: model.is_super,
        data_scope: model.data_scope,
        status: model.status,
        sort: model.sort,
        remark: model.remark,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}

fn to_entity(tenant_id: &str, record: RoleRecord) -> role::Model {
    role::Model {
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
        created_at: record.created_at,
        updated_at: record.updated_at,
    }
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}
