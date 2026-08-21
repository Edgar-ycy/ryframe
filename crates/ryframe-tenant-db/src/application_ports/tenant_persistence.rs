use std::sync::Arc;

use ryframe_application::{
    AuthorizationMirrorTransaction, PersistenceFuture, ProductTransactionPort,
    ProvisionTenantRecord, TenantAdminRecord, TenantPersistencePort, TenantProductAssignmentRecord,
    TenantProvisionRequestRecord, TenantProvisioningPlacement, TenantRecord, TenantTransaction,
};
use ryframe_db::{
    ControlDatabaseCluster, ProductRepository, ProvisionTenantCommand, ReadConsistency,
    TenantProvisioningRepository, TenantRepository, application_ports::DatabasePortTransaction,
    entities::tenant,
};
use ryframe_kernel::AppError;
use sea_orm::{ActiveModelTrait, IntoActiveModel, TransactionTrait};

use crate::TenantDataPlacementRepository;

use super::{map_error, provisioning::to_infrastructure_placement};

struct TenantPersistence {
    database: ControlDatabaseCluster,
}

struct TenantWorkUnit {
    transaction: DatabasePortTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn TenantPersistencePort> {
    Arc::new(TenantPersistence { database })
}

impl TenantPersistencePort for TenantPersistence {
    fn list(&self) -> PersistenceFuture<'_, Vec<TenantRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Eventual)
                .connection;
            TenantRepository
                .list_all(&database)
                .await
                .map(|records| records.into_iter().map(map_tenant).collect())
        })
    }

    fn find<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, Option<TenantRecord>> {
        Box::pin(async move {
            TenantRepository
                .find_by_tenant_id(self.database.write(), tenant_id)
                .await
                .map(|record| record.map(map_tenant))
        })
    }

    fn begin(&self) -> PersistenceFuture<'_, Box<dyn TenantTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(TenantWorkUnit {
                transaction: transaction.into(),
            }) as Box<dyn TenantTransaction>)
        })
    }
}

impl TenantTransaction for TenantWorkUnit {
    fn product(&self) -> &dyn ProductTransactionPort {
        &self.transaction
    }

    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction {
        &self.transaction
    }

    fn lock_optional_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantRecord>> {
        Box::pin(async move {
            TenantRepository
                .lock_optional_tenant_in_txn(&self.transaction, tenant_id)
                .await
                .map(|record| record.map(map_tenant))
        })
    }

    fn lock_tenant<'a>(&'a self, tenant_id: &'a str) -> PersistenceFuture<'a, TenantRecord> {
        Box::pin(async move {
            TenantRepository
                .lock_tenant_in_txn(&self.transaction, tenant_id)
                .await
                .map(map_tenant)
        })
    }

    fn lock_tenant_with_limits<'a>(
        &'a self,
        tenant_id: &'a str,
        max_users: i32,
        max_roles: i32,
        max_storage_mb: i64,
    ) -> PersistenceFuture<'a, TenantRecord> {
        Box::pin(async move {
            TenantRepository
                .lock_and_validate_resource_limits_in_txn(
                    &self.transaction,
                    tenant_id,
                    max_users,
                    max_roles,
                    max_storage_mb,
                )
                .await
                .map(map_tenant)
        })
    }

    fn lock_provision_request<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantProvisionRequestRecord>> {
        Box::pin(async move {
            TenantProvisioningRepository
                .lock_provision_request_in_txn(&self.transaction, tenant_id)
                .await
                .map(|record| {
                    record.map(|request| TenantProvisionRequestRecord {
                        request_token: request.request_token,
                        admin_password_hash: request.admin_password_hash,
                    })
                })
        })
    }

    fn provision(&self, record: ProvisionTenantRecord) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
            TenantProvisioningRepository
                .provision_in_transaction(&self.transaction, map_provision(record))
                .await
                .map(|_| ())
        })
    }

    fn assign_initial_product<'a>(
        &'a self,
        tenant_id: &'a str,
        plan_version_id: i64,
        changed_by: i64,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            ProductRepository
                .assign_initial_in_txn(&self.transaction, tenant_id, plan_version_id, changed_by)
                .await
                .map(|_| ())
        })
    }

    fn product_assignment<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantProductAssignmentRecord>> {
        Box::pin(async move {
            ProductRepository
                .assignment(&self.transaction, tenant_id)
                .await
                .map(|record| {
                    record.map(|assignment| TenantProductAssignmentRecord {
                        plan_version_id: assignment.plan_version_id,
                    })
                })
        })
    }

    fn find_admin<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantAdminRecord>> {
        Box::pin(async move {
            TenantProvisioningRepository
                .find_user_by_username(&self.transaction, tenant_id, username)
                .await
                .map(|record| {
                    record.map(|user| TenantAdminRecord {
                        password_hash: user.password_hash,
                    })
                })
        })
    }

    fn save_tenant(&self, tenant: TenantRecord) -> PersistenceFuture<'_, TenantRecord> {
        Box::pin(async move {
            map_tenant_model(tenant)
                .into_active_model()
                .reset_all()
                .update(&self.transaction)
                .await
                .map(map_tenant)
                .map_err(database_error)
        })
    }

    fn update_status<'a>(
        &'a self,
        tenant_id: &'a str,
        status: &'a str,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantRepository
                .update_status(&self.transaction, tenant_id, status)
                .await
        })
    }

    fn create_pending<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantDataPlacementRepository
                .create_pending(&self.transaction, &to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }

    fn create_or_resume_pending<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantDataPlacementRepository
                .create_or_resume_pending(
                    &self.transaction,
                    &to_infrastructure_placement(placement),
                )
                .await
                .map_err(map_error)
        })
    }

    fn activate_placement<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantDataPlacementRepository
                .activate(&self.transaction, &to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }

    fn fail_placement<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantDataPlacementRepository
                .fail(&self.transaction, &to_infrastructure_placement(placement))
                .await
                .map_err(map_error)
        })
    }

    fn commit_audited(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit_audited().await })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.commit().await.map_err(database_error) })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

fn map_tenant(tenant: tenant::Model) -> TenantRecord {
    TenantRecord {
        id: tenant.id,
        tenant_id: tenant.tenant_id,
        name: tenant.name,
        domain: tenant.domain,
        status: tenant.status,
        expire_at: tenant.expire_at,
        max_users: tenant.max_users,
        max_roles: tenant.max_roles,
        max_storage_mb: tenant.max_storage_mb,
        max_requests_per_min: tenant.max_requests_per_min,
        session_version: tenant.session_version,
        authorization_epoch: tenant.authorization_epoch,
        runtime_epoch: tenant.runtime_epoch,
        configuration_version: tenant.configuration_version,
        created_at: tenant.created_at,
        updated_at: tenant.updated_at,
    }
}

fn map_tenant_model(tenant: TenantRecord) -> tenant::Model {
    tenant::Model {
        id: tenant.id,
        tenant_id: tenant.tenant_id,
        name: tenant.name,
        domain: tenant.domain,
        status: tenant.status,
        expire_at: tenant.expire_at,
        max_users: tenant.max_users,
        max_roles: tenant.max_roles,
        max_storage_mb: tenant.max_storage_mb,
        max_requests_per_min: tenant.max_requests_per_min,
        session_version: tenant.session_version,
        authorization_epoch: tenant.authorization_epoch,
        runtime_epoch: tenant.runtime_epoch,
        configuration_version: tenant.configuration_version,
        created_at: tenant.created_at,
        updated_at: tenant.updated_at,
    }
}

fn map_provision(record: ProvisionTenantRecord) -> ProvisionTenantCommand {
    ProvisionTenantCommand {
        provisioning_request_token: record.provisioning_request_token,
        tenant_id: record.tenant_id,
        name: record.name,
        domain: record.domain,
        expire_at: record.expire_at,
        max_users: record.max_users,
        max_roles: record.max_roles,
        max_storage_mb: record.max_storage_mb,
        max_requests_per_minute: record.max_requests_per_minute,
        admin_username: record.admin_username,
        admin_password_hash: record.admin_password_hash,
        enabled_capability_route_keys: record.enabled_capability_route_keys,
        enabled_capability_permission_codes: record.enabled_capability_permission_codes,
        managed_capability_route_keys: record.managed_capability_route_keys,
        managed_capability_permission_codes: record.managed_capability_permission_codes,
        default_admin_permission_codes: record.default_admin_permission_codes,
    }
}

fn database_error(error: sea_orm::DbErr) -> AppError {
    AppError::Database(error.to_string())
}

#[cfg(test)]
mod tests {
    use ryframe_db::entities::tenant;

    use super::{map_tenant, map_tenant_model};

    #[test]
    fn tenant_mapping_preserves_every_field() {
        let now = "2026-08-21T01:02:03Z".parse().unwrap();
        let model = tenant::Model {
            id: 42,
            tenant_id: "tenant-a".to_owned(),
            name: "Tenant A".to_owned(),
            domain: Some("tenant.example.com".to_owned()),
            status: tenant::Model::STATUS_ENABLED.to_owned(),
            expire_at: None,
            max_users: 100,
            max_roles: 20,
            max_storage_mb: 1024,
            max_requests_per_min: 1000,
            session_version: 3,
            authorization_epoch: 4,
            runtime_epoch: 5,
            configuration_version: 6,
            created_at: now,
            updated_at: now,
        };

        assert_eq!(map_tenant_model(map_tenant(model.clone())), model);
    }
}
