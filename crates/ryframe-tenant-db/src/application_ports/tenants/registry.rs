use ryframe_db::DbResultExt;
use std::sync::Arc;

use ryframe_application::ports::{
    authorization::AuthorizationMirrorTransaction,
    product::ProductTransactionPort,
    tenants::{
        ProvisionTenantRecord, TenantAdminRecord, TenantAuthorizationTemplate,
        TenantBaseCatalogTemplate, TenantPersistencePort, TenantProductAssignmentRecord,
        TenantProvisionRequestRecord, TenantProvisioningIdentity, TenantProvisioningPlacement,
        TenantProvisioningTemplate, TenantRecord, TenantTransaction,
    },
};
use ryframe_db::{
    ControlDatabaseCluster, ProductRepository, ReadConsistency, TenantProvisioningRepository,
    TenantRepository, application_ports::transaction::DatabasePortTransaction, entities::tenant,
};
use sea_orm::{ActiveModelTrait, IntoActiveModel, TransactionTrait};

use crate::TenantDataPlacementRepository;

use super::super::map_error;
use super::provisioning::to_infrastructure_placement;

struct TenantPersistence {
    database: ControlDatabaseCluster,
}

struct TenantWorkUnit {
    transaction: DatabasePortTransaction,
}

pub fn port(database: ControlDatabaseCluster) -> Arc<dyn TenantPersistencePort> {
    Arc::new(TenantPersistence { database })
}

#[async_trait::async_trait]
impl TenantPersistencePort for TenantPersistence {
    async fn list(&self) -> ryframe_kernel::AppResult<Vec<TenantRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Eventual)
            .connection;
        TenantRepository
            .list_all(&database)
            .await
            .map(|records| records.into_iter().map(map_tenant).collect())
    }

    async fn find<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantRecord>> {
        TenantRepository
            .find_by_tenant_id(self.database.write(), tenant_id)
            .await
            .map(|record| record.map(map_tenant))
    }

    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn TenantTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(TenantWorkUnit {
            transaction: transaction.into(),
        }) as Box<dyn TenantTransaction>)
    }
}

#[async_trait::async_trait]
impl TenantTransaction for TenantWorkUnit {
    fn product(&self) -> &dyn ProductTransactionPort {
        &self.transaction
    }

    fn authorization_mirror(&self) -> &dyn AuthorizationMirrorTransaction {
        &self.transaction
    }

    async fn lock_optional_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantRecord>> {
        TenantRepository
            .lock_optional_tenant_in_txn(&self.transaction, tenant_id)
            .await
            .map(|record| record.map(map_tenant))
    }

    async fn lock_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<TenantRecord> {
        TenantRepository
            .lock_tenant_in_txn(&self.transaction, tenant_id)
            .await
            .map(map_tenant)
    }

    async fn lock_tenant_with_limits<'a>(
        &'a self,
        tenant_id: &'a str,
        max_users: i32,
        max_roles: i32,
        max_storage_mb: i64,
    ) -> ryframe_kernel::AppResult<TenantRecord> {
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
    }

    async fn lock_provision_request<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantProvisionRequestRecord>> {
        TenantProvisioningRepository
            .lock_provision_request_in_txn(&self.transaction, tenant_id)
            .await
            .map(|record| {
                record.map(|request| TenantProvisionRequestRecord {
                    request_token: request.request_token,
                    admin_password_hash: request.admin_password_hash,
                })
            })
    }

    async fn load_provisioning_template(
        &self,
    ) -> ryframe_kernel::AppResult<TenantProvisioningTemplate> {
        TenantProvisioningRepository
            .load_template_in_transaction(&self.transaction)
            .await
    }

    async fn initialize_tenant_identity<'a>(
        &'a self,
        record: &'a ProvisionTenantRecord,
    ) -> ryframe_kernel::AppResult<TenantProvisioningIdentity> {
        TenantProvisioningRepository
            .initialize_identity_in_transaction(&self.transaction, record)
            .await
    }

    async fn copy_tenant_authorization<'a>(
        &'a self,
        identity: &'a TenantProvisioningIdentity,
        template: TenantAuthorizationTemplate,
    ) -> ryframe_kernel::AppResult<()> {
        TenantProvisioningRepository
            .copy_authorization_in_transaction(&self.transaction, identity, template)
            .await
    }

    async fn copy_tenant_base_catalogs<'a>(
        &'a self,
        identity: &'a TenantProvisioningIdentity,
        template: TenantBaseCatalogTemplate,
    ) -> ryframe_kernel::AppResult<()> {
        TenantProvisioningRepository
            .copy_base_catalogs_in_transaction(&self.transaction, identity, template)
            .await
    }

    async fn assign_initial_product<'a>(
        &'a self,
        tenant_id: &'a str,
        plan_version_id: i64,
        changed_by: i64,
    ) -> ryframe_kernel::AppResult<()> {
        ProductRepository
            .assign_initial_in_txn(&self.transaction, tenant_id, plan_version_id, changed_by)
            .await
            .map(|_| ())
    }

    async fn product_assignment<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantProductAssignmentRecord>> {
        ProductRepository
            .assignment(&self.transaction, tenant_id)
            .await
            .map(|record| {
                record.map(|assignment| TenantProductAssignmentRecord {
                    plan_version_id: assignment.plan_version_id,
                })
            })
    }

    async fn find_admin<'a>(
        &'a self,
        tenant_id: &'a str,
        username: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantAdminRecord>> {
        TenantProvisioningRepository
            .find_user_by_username(&self.transaction, tenant_id, username)
            .await
            .map(|record| {
                record.map(|user| TenantAdminRecord {
                    password_hash: user.password_hash,
                })
            })
    }

    async fn save_tenant(&self, tenant: TenantRecord) -> ryframe_kernel::AppResult<TenantRecord> {
        map_tenant_model(tenant)
            .into_active_model()
            .reset_all()
            .update(&self.transaction)
            .await
            .map(map_tenant)
            .db()
    }

    async fn update_status<'a>(
        &'a self,
        tenant_id: &'a str,
        status: &'a str,
    ) -> ryframe_kernel::AppResult<()> {
        TenantRepository
            .update_status(&self.transaction, tenant_id, status)
            .await
    }

    async fn create_pending<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> ryframe_kernel::AppResult<()> {
        TenantDataPlacementRepository
            .create_pending(&self.transaction, &to_infrastructure_placement(placement))
            .await
            .map_err(map_error)
    }

    async fn create_or_resume_pending<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> ryframe_kernel::AppResult<()> {
        TenantDataPlacementRepository
            .create_or_resume_pending(&self.transaction, &to_infrastructure_placement(placement))
            .await
            .map_err(map_error)
    }

    async fn activate_placement<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> ryframe_kernel::AppResult<()> {
        TenantDataPlacementRepository
            .activate(&self.transaction, &to_infrastructure_placement(placement))
            .await
            .map_err(map_error)
    }

    async fn fail_placement<'a>(
        &'a self,
        placement: &'a TenantProvisioningPlacement,
    ) -> ryframe_kernel::AppResult<()> {
        TenantDataPlacementRepository
            .fail(&self.transaction, &to_infrastructure_placement(placement))
            .await
            .map_err(map_error)
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for TenantWorkUnit {
    async fn commit(
        self: Box<Self>,
        audit_mode: ryframe_application::TransactionAuditMode,
    ) -> ryframe_kernel::AppResult<()> {
        match audit_mode {
            ryframe_application::TransactionAuditMode::CurrentRequest => {
                self.transaction.commit_audited().await
            }
            ryframe_application::TransactionAuditMode::Skip => self.transaction.commit().await.db(),
        }
    }

    async fn rollback(self: Box<Self>) -> ryframe_kernel::AppResult<()> {
        self.transaction.rollback().await.db()
    }
}

pub fn map_tenant(tenant: tenant::Model) -> TenantRecord {
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

pub fn map_tenant_model(tenant: TenantRecord) -> tenant::Model {
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
