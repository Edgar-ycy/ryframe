use std::sync::Arc;

use ryframe_db::{ControlDatabaseCluster, ProductRepository, ReadConsistency};

use crate::{
    PersistenceFuture, ProductCapabilityRecord, ProductPlanRecord, ProductReadPort,
    ProductVersionRecord, ProductVersionSnapshot, TenantCapabilityOverrideRecord,
    TenantProductSnapshot,
};

pub fn read_port(database: ControlDatabaseCluster) -> Arc<dyn ProductReadPort> {
    Arc::new(LegacyProductRead { database })
}

struct LegacyProductRead {
    database: ControlDatabaseCluster,
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
