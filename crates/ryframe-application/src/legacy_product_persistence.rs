use std::sync::Arc;

use ryframe_db::{
    ControlDatabaseCluster, ProductRepository, ReadConsistency,
    entities::{product_plan, product_plan_capability, product_plan_version},
};
use sea_orm::{DatabaseTransaction, TransactionTrait};

use crate::{
    PersistenceFuture, ProductCapabilityRecord, ProductPlanRecord, ProductPlanState,
    ProductReadPort, ProductVersionRecord, ProductVersionSnapshot, ProductVersionState,
    ProductVersionWriteResult, ProductWritePort, ProductWriteTransaction,
    TenantCapabilityOverrideRecord, TenantProductSnapshot,
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
