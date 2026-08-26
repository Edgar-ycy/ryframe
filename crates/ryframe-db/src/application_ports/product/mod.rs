mod capabilities;
mod records;

use crate::DbResultExt;
use std::sync::Arc;

use crate::{
    ControlDatabaseCluster, ProductRepository, ReadConsistency, TenantOperationLeaseRepository,
    entities::tenant::operation_lease as tenant_operation_lease,
};
use chrono::{DateTime, Utc};
use ryframe_kernel::AppError;
use sea_orm::TransactionTrait;

use ryframe_application::ports::product::{
    ProductAssignmentChange, ProductCapabilityRecord, ProductChangeTenantState, ProductPlanRecord,
    ProductPlanState, ProductReadPort, ProductTransactionPort, ProductVersionSnapshot,
    ProductVersionState, ProductVersionWriteResult, ProductWritePort, ProductWriteTransaction,
    ProvisioningCapabilityResources, TenantProductSnapshot,
};

use super::transaction::DatabasePortTransaction;
use capabilities::sync_persisted_capability_resources;
use records::{
    capability_models, override_models, plan_model, plan_record, plan_state, version_model,
    version_state,
};
pub(crate) use records::{capability_record, tenant_snapshot, version_snapshot};

pub fn read(database: ControlDatabaseCluster) -> Arc<dyn ProductReadPort> {
    Arc::new(DatabaseProductRead { database })
}

pub fn write(database: ControlDatabaseCluster) -> Arc<dyn ProductWritePort> {
    Arc::new(DatabaseProductWrite { database })
}

struct DatabaseProductRead {
    database: ControlDatabaseCluster,
}

struct DatabaseProductWrite {
    database: ControlDatabaseCluster,
}

struct DatabaseProductWriteTransaction {
    transaction: DatabasePortTransaction,
}

#[async_trait::async_trait]
impl ProductTransactionPort for DatabasePortTransaction {
    async fn current_tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<TenantProductSnapshot> {
        ProductRepository
            .tenant_product(self, tenant_id)
            .await?
            .map(tenant_snapshot)
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))
    }

    async fn lock_assignable_version(
        &self,
        version_id: i64,
    ) -> ryframe_kernel::AppResult<ProductVersionSnapshot> {
        let repository = ProductRepository;
        let observed = repository
            .find_version_by_id(self, version_id)
            .await?
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))?;
        let plan = repository
            .lock_plan_by_id_in_txn(self, observed.plan.id)
            .await?;
        let version = repository
            .lock_version_by_id_in_txn(self, version_id)
            .await?;
        if version.plan_id != plan.id {
            return Err(AppError::Conflict(
                "目标产品套餐版本所属套餐已变化，请重新预览".into(),
            ));
        }
        repository
            .find_version_by_id(self, version_id)
            .await?
            .map(version_snapshot)
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))
    }

    async fn sync_capability_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a ProvisioningCapabilityResources,
    ) -> ryframe_kernel::AppResult<()> {
        sync_persisted_capability_resources(self, tenant_id, resources).await
    }
}

#[async_trait::async_trait]
impl ProductReadPort for DatabaseProductRead {
    async fn list_plans(&self) -> ryframe_kernel::AppResult<Vec<ProductPlanRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        let repository = ProductRepository;
        let plans = repository.list_plans(&database).await?;
        let mut records = Vec::with_capacity(plans.len());
        for plan in plans {
            records.push(plan_record(&repository, &database, plan).await?);
        }
        Ok(records)
    }

    async fn find_plan(
        &self,
        plan_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProductPlanRecord>> {
        let database = self
            .database
            .select_read(ReadConsistency::Strong)
            .connection;
        let repository = ProductRepository;
        let Some(plan) = repository.find_plan_by_id(&database, plan_id).await? else {
            return Ok(None);
        };
        plan_record(&repository, &database, plan).await.map(Some)
    }

    async fn find_version(
        &self,
        version_id: i64,
    ) -> ryframe_kernel::AppResult<Option<ProductVersionSnapshot>> {
        ProductRepository
            .find_version_by_id(self.database.write(), version_id)
            .await
            .map(|bundle| bundle.map(version_snapshot))
    }

    async fn tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<Option<TenantProductSnapshot>> {
        ProductRepository
            .tenant_product(self.database.write(), tenant_id)
            .await
            .map(|bundle| bundle.map(tenant_snapshot))
    }
}

#[async_trait::async_trait]
impl ProductWritePort for DatabaseProductWrite {
    async fn begin(&self) -> ryframe_kernel::AppResult<Box<dyn ProductWriteTransaction>> {
        let transaction = self.database.write().begin().await.db()?;
        Ok(Box::new(DatabaseProductWriteTransaction {
            transaction: transaction.into(),
        }) as Box<dyn ProductWriteTransaction>)
    }
}

#[async_trait::async_trait]
impl ProductWriteTransaction for DatabaseProductWriteTransaction {
    async fn lock_change_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<ProductChangeTenantState> {
        let repository = TenantOperationLeaseRepository;
        let tenant = repository
            .lock_tenant_and_validate_in_txn(&self.transaction, tenant_id, None)
            .await?;
        let database_now = crate::repositories::database_utc_now(&self.transaction).await?;
        Ok(ProductChangeTenantState {
            status: tenant.status,
            authorization_epoch: tenant.authorization_epoch,
            runtime_epoch: tenant.runtime_epoch,
            database_now,
        })
    }

    async fn acquire_change_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        version_id: i64,
        acquired_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<()> {
        TenantOperationLeaseRepository
            .acquire_in_txn(
                &self.transaction,
                tenant_operation_lease::Model {
                    tenant_id: tenant_id.to_owned(),
                    owner_token: owner_token.to_owned(),
                    operation: "product.change".into(),
                    resource_type: "product_plan_version".into(),
                    resource_id: version_id.to_string(),
                    expires_at,
                    created_at: acquired_at,
                    updated_at: acquired_at,
                },
            )
            .await
            .map(|_| ())
    }

    async fn lock_assignable_version(
        &self,
        version_id: i64,
    ) -> ryframe_kernel::AppResult<ProductVersionSnapshot> {
        let repository = ProductRepository;
        let observed = repository
            .find_version_by_id(&self.transaction, version_id)
            .await?
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))?;
        let plan = repository
            .lock_plan_by_id_in_txn(&self.transaction, observed.plan.id)
            .await?;
        let version = repository
            .lock_version_by_id_in_txn(&self.transaction, version_id)
            .await?;
        if version.plan_id != plan.id {
            return Err(AppError::Conflict(
                "目标产品套餐版本所属套餐已变化，请重新预览".into(),
            ));
        }
        repository
            .find_version_by_id(&self.transaction, version_id)
            .await?
            .map(version_snapshot)
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))
    }

    async fn current_tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> ryframe_kernel::AppResult<TenantProductSnapshot> {
        self.transaction.current_tenant_product(tenant_id).await
    }

    async fn sync_capability_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a ProvisioningCapabilityResources,
    ) -> ryframe_kernel::AppResult<()> {
        self.transaction
            .sync_capability_resources(tenant_id, resources)
            .await
    }

    async fn replace_assignment(
        &self,
        change: ProductAssignmentChange,
    ) -> ryframe_kernel::AppResult<()> {
        let ProductAssignmentChange {
            tenant_id,
            version_id,
            changed_by,
            reason,
            overrides,
            changed_at,
        } = change;
        let repository = ProductRepository;
        let mut assignment = repository
            .lock_assignment_in_txn(&self.transaction, &tenant_id)
            .await?;
        assignment.plan_version_id = version_id;
        assignment.changed_by = Some(changed_by);
        assignment.change_reason = reason;
        assignment.updated_at = changed_at;
        repository
            .replace_assignment_and_overrides_in_txn(
                &self.transaction,
                assignment,
                override_models(&tenant_id, changed_at, overrides),
            )
            .await
    }

    async fn increment_runtime_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
        expected_epoch: i64,
    ) -> ryframe_kernel::AppResult<()> {
        ProductRepository
            .increment_runtime_epoch_in_txn(&self.transaction, tenant_id, expected_epoch)
            .await
            .map(|_| ())
    }

    fn authorization_mirror(
        &self,
    ) -> &dyn ryframe_application::ports::authorization::AuthorizationMirrorTransaction {
        &self.transaction
    }

    async fn release_change_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> ryframe_kernel::AppResult<()> {
        TenantOperationLeaseRepository
            .release_in_txn(&self.transaction, tenant_id, owner_token)
            .await
            .map(|_| ())
    }

    async fn plan_key_exists<'a>(&'a self, key: &'a str) -> ryframe_kernel::AppResult<bool> {
        ProductRepository
            .find_plan_by_key(&self.transaction, key)
            .await
            .map(|plan| plan.is_some())
    }

    async fn insert_plan(
        &self,
        plan: ProductPlanState,
    ) -> ryframe_kernel::AppResult<ProductPlanState> {
        ProductRepository
            .insert_plan_in_txn(&self.transaction, plan_model(plan))
            .await
            .map(plan_state)
    }

    async fn lock_plan(&self, plan_id: i64) -> ryframe_kernel::AppResult<ProductPlanState> {
        ProductRepository
            .lock_plan_by_id_in_txn(&self.transaction, plan_id)
            .await
            .map(plan_state)
    }

    async fn save_plan(
        &self,
        plan: ProductPlanState,
    ) -> ryframe_kernel::AppResult<ProductPlanState> {
        ProductRepository
            .update_plan_in_txn(&self.transaction, plan_model(plan))
            .await
            .map(plan_state)
    }

    async fn next_version(&self, plan_id: i64) -> ryframe_kernel::AppResult<i32> {
        ProductRepository
            .next_version_in_txn(&self.transaction, plan_id)
            .await
    }

    async fn insert_version(
        &self,
        version: ProductVersionState,
        capabilities: Vec<ProductCapabilityRecord>,
        capability_time: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<ProductVersionWriteResult> {
        let version_id = version.id;
        let saved = ProductRepository
            .insert_version_in_txn(
                &self.transaction,
                version_model(version),
                capability_models(version_id, capabilities, capability_time),
            )
            .await?;
        let capabilities = ProductRepository
            .list_capabilities(&self.transaction, version_id)
            .await?
            .into_iter()
            .map(capability_record)
            .collect();
        Ok(ProductVersionWriteResult {
            version: version_state(saved),
            capabilities,
        })
    }

    async fn lock_version(
        &self,
        plan_id: i64,
        version: i32,
    ) -> ryframe_kernel::AppResult<ProductVersionState> {
        ProductRepository
            .lock_version_in_txn(&self.transaction, plan_id, version)
            .await
            .map(version_state)
    }

    async fn capabilities(
        &self,
        version_id: i64,
    ) -> ryframe_kernel::AppResult<Vec<ProductCapabilityRecord>> {
        ProductRepository
            .list_capabilities(&self.transaction, version_id)
            .await
            .map(|items| items.into_iter().map(capability_record).collect())
    }

    async fn replace_draft_version(
        &self,
        version: ProductVersionState,
        capabilities: Vec<ProductCapabilityRecord>,
        capability_time: DateTime<Utc>,
    ) -> ryframe_kernel::AppResult<ProductVersionWriteResult> {
        let version_id = version.id;
        let saved = ProductRepository
            .replace_draft_version_in_txn(
                &self.transaction,
                version_model(version),
                capability_models(version_id, capabilities, capability_time),
            )
            .await?;
        let capabilities = ProductRepository
            .list_capabilities(&self.transaction, version_id)
            .await?
            .into_iter()
            .map(capability_record)
            .collect();
        Ok(ProductVersionWriteResult {
            version: version_state(saved),
            capabilities,
        })
    }

    async fn transition_version(
        &self,
        version: ProductVersionState,
        expected_status: &str,
        target_status: &str,
    ) -> ryframe_kernel::AppResult<ProductVersionState> {
        let expected_status = expected_status.to_owned();
        let target_status = target_status.to_owned();
        ProductRepository
            .transition_version_status_in_txn(
                &self.transaction,
                version_model(version),
                &expected_status,
                &target_status,
            )
            .await
            .map(version_state)
    }
}

#[async_trait::async_trait]
impl ryframe_application::PersistenceTransaction for DatabaseProductWriteTransaction {
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
