use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, ProductRepository, ReadConsistency, TenantOperationLeaseRepository,
    entities::{
        product_plan, product_plan_capability, product_plan_version, tenant_capability_override,
        tenant_operation_lease,
    },
};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use crate::{
    PersistenceFuture, ProductAssignmentChange, ProductCapabilityRecord, ProductChangeTenantState,
    ProductPlanRecord, ProductPlanState, ProductReadPort, ProductVersionRecord,
    ProductVersionSnapshot, ProductVersionState, ProductVersionWriteResult, ProductWritePort,
    ProductWriteTransaction, ProvisioningCapabilityResources, TenantCapabilityOverrideRecord,
    TenantProductSnapshot,
};

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn ProductReadPort> {
    Arc::new(LegacyProductRead { database })
}

pub fn write_port(database: ControlDatabaseCluster) -> Arc<dyn ProductWritePort> {
    Arc::new(LegacyProductWrite { database })
}

struct LegacyProductRead {
    database: ControlDatabaseCluster,
}

struct LegacyProductWrite {
    database: ControlDatabaseCluster,
}

struct LegacyProductWriteTransaction {
    transaction: DatabaseTransaction,
}

impl ProductReadPort for LegacyProductRead {
    fn list_plans(&self) -> PersistenceFuture<'_, Vec<ProductPlanRecord>> {
        Box::pin(async move {
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
        })
    }

    fn find_plan(&self, plan_id: i64) -> PersistenceFuture<'_, Option<ProductPlanRecord>> {
        Box::pin(async move {
            let database = self
                .database
                .select_read(ReadConsistency::Strong)
                .connection;
            let repository = ProductRepository;
            let Some(plan) = repository.find_plan_by_id(&database, plan_id).await? else {
                return Ok(None);
            };
            plan_record(&repository, &database, plan).await.map(Some)
        })
    }

    fn find_version(
        &self,
        version_id: i64,
    ) -> PersistenceFuture<'_, Option<ProductVersionSnapshot>> {
        Box::pin(async move {
            ProductRepository
                .find_version_by_id(self.database.write(), version_id)
                .await
                .map(|bundle| bundle.map(version_snapshot))
        })
    }

    fn tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, Option<TenantProductSnapshot>> {
        Box::pin(async move {
            ProductRepository
                .tenant_product(self.database.write(), tenant_id)
                .await
                .map(|bundle| bundle.map(tenant_snapshot))
        })
    }
}

impl ProductWritePort for LegacyProductWrite {
    fn begin(&self) -> PersistenceFuture<'_, Box<dyn ProductWriteTransaction>> {
        Box::pin(async move {
            let transaction = self
                .database
                .write()
                .begin()
                .await
                .map_err(database_error)?;
            Ok(Box::new(LegacyProductWriteTransaction { transaction })
                as Box<dyn ProductWriteTransaction>)
        })
    }
}

impl ProductWriteTransaction for LegacyProductWriteTransaction {
    fn lock_change_tenant<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, ProductChangeTenantState> {
        Box::pin(async move {
            let repository = TenantOperationLeaseRepository;
            let tenant = repository
                .lock_tenant_and_validate_in_txn(&self.transaction, tenant_id, None)
                .await?;
            let database_now = repository.database_utc_now(&self.transaction).await?;
            Ok(ProductChangeTenantState {
                status: tenant.status,
                authorization_epoch: tenant.authorization_epoch,
                runtime_epoch: tenant.runtime_epoch,
                database_now,
            })
        })
    }

    fn acquire_change_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
        version_id: i64,
        acquired_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
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
        })
    }

    fn lock_assignable_version(
        &self,
        version_id: i64,
    ) -> PersistenceFuture<'_, ProductVersionSnapshot> {
        Box::pin(async move {
            let repository = ProductRepository;
            let observed = repository
                .find_version_by_id(&self.transaction, version_id)
                .await?
                .ok_or_else(|| {
                    ryframe_kernel::AppError::NotFound("目标产品套餐版本不存在".into())
                })?;
            let plan = repository
                .lock_plan_by_id_in_txn(&self.transaction, observed.plan.id)
                .await?;
            let version = repository
                .lock_version_by_id_in_txn(&self.transaction, version_id)
                .await?;
            if version.plan_id != plan.id {
                return Err(ryframe_kernel::AppError::Conflict(
                    "目标产品套餐版本所属套餐已变化，请重新预览".into(),
                ));
            }
            repository
                .find_version_by_id(&self.transaction, version_id)
                .await?
                .map(version_snapshot)
                .ok_or_else(|| ryframe_kernel::AppError::NotFound("目标产品套餐版本不存在".into()))
        })
    }

    fn current_tenant_product<'a>(
        &'a self,
        tenant_id: &'a str,
    ) -> PersistenceFuture<'a, TenantProductSnapshot> {
        Box::pin(async move {
            ProductRepository
                .tenant_product(&self.transaction, tenant_id)
                .await?
                .map(tenant_snapshot)
                .ok_or_else(|| ryframe_kernel::AppError::NotFound("租户不存在".into()))
        })
    }

    fn sync_capability_resources<'a>(
        &'a self,
        tenant_id: &'a str,
        resources: &'a ProvisioningCapabilityResources,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            crate::system::product_service::resources::sync_persisted_capability_resources(
                &self.transaction,
                tenant_id,
                resources,
            )
            .await
        })
    }

    fn replace_assignment(&self, change: ProductAssignmentChange) -> PersistenceFuture<'_, ()> {
        Box::pin(async move {
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
        })
    }

    fn increment_runtime_epoch<'a>(
        &'a self,
        tenant_id: &'a str,
        expected_epoch: i64,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            ProductRepository
                .increment_runtime_epoch_in_txn(&self.transaction, tenant_id, expected_epoch)
                .await
                .map(|_| ())
        })
    }

    fn authorization_mirror(&self) -> &dyn crate::AuthorizationMirrorTransaction {
        &self.transaction
    }

    fn release_change_lease<'a>(
        &'a self,
        tenant_id: &'a str,
        owner_token: &'a str,
    ) -> PersistenceFuture<'a, ()> {
        Box::pin(async move {
            TenantOperationLeaseRepository
                .release_in_txn(&self.transaction, tenant_id, owner_token)
                .await
                .map(|_| ())
        })
    }

    fn plan_key_exists<'a>(&'a self, key: &'a str) -> PersistenceFuture<'a, bool> {
        Box::pin(async move {
            ProductRepository
                .find_plan_by_key(&self.transaction, key)
                .await
                .map(|plan| plan.is_some())
        })
    }

    fn insert_plan(&self, plan: ProductPlanState) -> PersistenceFuture<'_, ProductPlanState> {
        Box::pin(async move {
            ProductRepository
                .insert_plan_in_txn(&self.transaction, plan_model(plan))
                .await
                .map(plan_state)
        })
    }

    fn lock_plan(&self, plan_id: i64) -> PersistenceFuture<'_, ProductPlanState> {
        Box::pin(async move {
            ProductRepository
                .lock_plan_by_id_in_txn(&self.transaction, plan_id)
                .await
                .map(plan_state)
        })
    }

    fn save_plan(&self, plan: ProductPlanState) -> PersistenceFuture<'_, ProductPlanState> {
        Box::pin(async move {
            ProductRepository
                .update_plan_in_txn(&self.transaction, plan_model(plan))
                .await
                .map(plan_state)
        })
    }

    fn next_version(&self, plan_id: i64) -> PersistenceFuture<'_, i32> {
        Box::pin(async move {
            ProductRepository
                .next_version_in_txn(&self.transaction, plan_id)
                .await
        })
    }

    fn insert_version(
        &self,
        version: ProductVersionState,
        capabilities: Vec<ProductCapabilityRecord>,
        capability_time: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, ProductVersionWriteResult> {
        Box::pin(async move {
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
        })
    }

    fn lock_version(
        &self,
        plan_id: i64,
        version: i32,
    ) -> PersistenceFuture<'_, ProductVersionState> {
        Box::pin(async move {
            ProductRepository
                .lock_version_in_txn(&self.transaction, plan_id, version)
                .await
                .map(version_state)
        })
    }

    fn capabilities(&self, version_id: i64) -> PersistenceFuture<'_, Vec<ProductCapabilityRecord>> {
        Box::pin(async move {
            ProductRepository
                .list_capabilities(&self.transaction, version_id)
                .await
                .map(|items| items.into_iter().map(capability_record).collect())
        })
    }

    fn replace_draft_version(
        &self,
        version: ProductVersionState,
        capabilities: Vec<ProductCapabilityRecord>,
        capability_time: chrono::DateTime<chrono::Utc>,
    ) -> PersistenceFuture<'_, ProductVersionWriteResult> {
        Box::pin(async move {
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
        })
    }

    fn transition_version(
        &self,
        version: ProductVersionState,
        expected_status: &str,
        target_status: &str,
    ) -> PersistenceFuture<'_, ProductVersionState> {
        let expected_status = expected_status.to_owned();
        let target_status = target_status.to_owned();
        Box::pin(async move {
            ProductRepository
                .transition_version_status_in_txn(
                    &self.transaction,
                    version_model(version),
                    &expected_status,
                    &target_status,
                )
                .await
                .map(version_state)
        })
    }

    fn commit(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { crate::commit_current_audit(self.transaction).await })
    }

    fn rollback(self: Box<Self>) -> PersistenceFuture<'static, ()> {
        Box::pin(async move { self.transaction.rollback().await.map_err(database_error) })
    }
}

async fn plan_record(
    repository: &ProductRepository,
    database: &sea_orm::DatabaseConnection,
    plan: ryframe_db::entities::product_plan::Model,
) -> ryframe_kernel::AppResult<ProductPlanRecord> {
    let versions = repository.list_versions(database, plan.id).await?;
    let mut version_records = Vec::with_capacity(versions.len());
    for version in versions {
        let capabilities = repository
            .list_capabilities(database, version.id)
            .await?
            .into_iter()
            .map(capability_record)
            .collect();
        version_records.push(ProductVersionRecord {
            id: version.id,
            version: version.version,
            name: version.name,
            description: version.description,
            status: version.status,
            created_by: version.created_by,
            published_by: version.published_by,
            published_at: version.published_at,
            capabilities,
        });
    }
    Ok(ProductPlanRecord {
        id: plan.id,
        key: plan.plan_key,
        name: plan.name,
        description: plan.description,
        status: plan.status,
        created_by: plan.created_by,
        versions: version_records,
    })
}

fn capability_record(
    capability: ryframe_db::entities::product_plan_capability::Model,
) -> ProductCapabilityRecord {
    ProductCapabilityRecord {
        code: capability.capability_code,
        variant: capability.variant_code,
        schema_version: capability.schema_version,
        config: capability.config,
    }
}

fn plan_state(plan: product_plan::Model) -> ProductPlanState {
    ProductPlanState {
        id: plan.id,
        key: plan.plan_key,
        name: plan.name,
        description: plan.description,
        status: plan.status,
        created_by: plan.created_by,
        created_at: plan.created_at,
        updated_at: plan.updated_at,
    }
}

fn plan_model(plan: ProductPlanState) -> product_plan::Model {
    product_plan::Model {
        id: plan.id,
        plan_key: plan.key,
        name: plan.name,
        description: plan.description,
        status: plan.status,
        created_by: plan.created_by,
        created_at: plan.created_at,
        updated_at: plan.updated_at,
    }
}

fn version_state(version: product_plan_version::Model) -> ProductVersionState {
    ProductVersionState {
        id: version.id,
        plan_id: version.plan_id,
        version: version.version,
        name: version.name,
        description: version.description,
        status: version.status,
        created_by: version.created_by,
        published_by: version.published_by,
        published_at: version.published_at,
        created_at: version.created_at,
        updated_at: version.updated_at,
    }
}

fn version_model(version: ProductVersionState) -> product_plan_version::Model {
    product_plan_version::Model {
        id: version.id,
        plan_id: version.plan_id,
        version: version.version,
        name: version.name,
        description: version.description,
        status: version.status,
        created_by: version.created_by,
        published_by: version.published_by,
        published_at: version.published_at,
        created_at: version.created_at,
        updated_at: version.updated_at,
    }
}

fn capability_models(
    version_id: i64,
    capabilities: Vec<ProductCapabilityRecord>,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<product_plan_capability::Model> {
    capabilities
        .into_iter()
        .map(|capability| product_plan_capability::Model {
            plan_version_id: version_id,
            capability_code: capability.code,
            variant_code: capability.variant,
            schema_version: capability.schema_version,
            config: capability.config,
            created_at: now,
            updated_at: now,
        })
        .collect()
}

fn override_models(
    tenant_id: &str,
    changed_at: chrono::DateTime<chrono::Utc>,
    overrides: Vec<TenantCapabilityOverrideRecord>,
) -> Vec<tenant_capability_override::Model> {
    overrides
        .into_iter()
        .map(|value| tenant_capability_override::Model {
            tenant_id: tenant_id.to_owned(),
            capability_code: value.code,
            enabled: value.enabled,
            variant_code: value.variant,
            schema_version: value.schema_version,
            config: value.config,
            reason: value.reason,
            changed_by: value.changed_by,
            created_at: changed_at,
            updated_at: changed_at,
        })
        .collect()
}

fn database_error(error: impl std::fmt::Display) -> ryframe_kernel::AppError {
    ryframe_kernel::AppError::Database(error.to_string())
}

pub(crate) fn version_snapshot(
    bundle: ryframe_db::ProductPlanVersionBundle,
) -> ProductVersionSnapshot {
    ProductVersionSnapshot {
        plan_key: bundle.plan.plan_key,
        plan_name: bundle.plan.name,
        plan_status: bundle.plan.status,
        version_id: bundle.version.id,
        version: bundle.version.version,
        version_status: bundle.version.status,
        capabilities: bundle
            .capabilities
            .into_iter()
            .map(capability_record)
            .collect(),
    }
}

pub(crate) fn tenant_snapshot(bundle: ryframe_db::TenantProductBundle) -> TenantProductSnapshot {
    TenantProductSnapshot {
        tenant_id: bundle.tenant.tenant_id,
        authorization_epoch: bundle.tenant.authorization_epoch,
        runtime_epoch: bundle.tenant.runtime_epoch,
        version: version_snapshot(ryframe_db::ProductPlanVersionBundle {
            plan: bundle.plan,
            version: bundle.version,
            capabilities: bundle.capabilities,
        }),
        overrides: bundle
            .overrides
            .into_iter()
            .map(|value| TenantCapabilityOverrideRecord {
                code: value.capability_code,
                enabled: value.enabled,
                variant: value.variant_code,
                schema_version: value.schema_version,
                config: value.config,
                reason: value.reason,
                changed_by: value.changed_by,
            })
            .collect(),
    }
}
