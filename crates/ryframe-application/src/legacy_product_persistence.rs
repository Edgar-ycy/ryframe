use std::sync::Arc;

use ryframe_db::{ControlDatabaseCluster, ProductRepository, ReadConsistency};

use crate::{
    PersistenceFuture, ProductCapabilityRecord, ProductPlanRecord, ProductReadPort,
    ProductVersionRecord,
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
            .map(|capability| ProductCapabilityRecord {
                code: capability.capability_code,
                variant: capability.variant_code,
                schema_version: capability.schema_version,
                config: capability.config,
            })
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
