use chrono::{DateTime, Utc};
use ryframe_application::ports::product::{
    ProductCapabilityRecord, ProductPlanRecord, ProductPlanState, ProductVersionRecord,
    ProductVersionSnapshot, ProductVersionState, TenantCapabilityOverrideRecord,
    TenantProductSnapshot,
};

use crate::{
    ProductRepository,
    entities::{
        product_plan, product_plan_capability, product_plan_version,
        tenant::capability_override as tenant_capability_override,
    },
};

pub(super) async fn plan_record(
    repository: &ProductRepository,
    database: &sea_orm::DatabaseConnection,
    plan: product_plan::Model,
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

pub(crate) fn capability_record(
    capability: product_plan_capability::Model,
) -> ProductCapabilityRecord {
    ProductCapabilityRecord {
        code: capability.capability_code,
        variant: capability.variant_code,
        schema_version: capability.schema_version,
        config: capability.config,
    }
}

pub(super) fn plan_state(plan: product_plan::Model) -> ProductPlanState {
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

pub(super) fn plan_model(plan: ProductPlanState) -> product_plan::Model {
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

pub(super) fn version_state(version: product_plan_version::Model) -> ProductVersionState {
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

pub(super) fn version_model(version: ProductVersionState) -> product_plan_version::Model {
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

pub(super) fn capability_models(
    version_id: i64,
    capabilities: Vec<ProductCapabilityRecord>,
    now: DateTime<Utc>,
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

pub(super) fn override_models(
    tenant_id: &str,
    changed_at: DateTime<Utc>,
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

pub(crate) fn version_snapshot(bundle: crate::ProductPlanVersionBundle) -> ProductVersionSnapshot {
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

pub(crate) fn tenant_snapshot(bundle: crate::TenantProductBundle) -> TenantProductSnapshot {
    TenantProductSnapshot {
        tenant_id: bundle.tenant.tenant_id,
        authorization_epoch: bundle.tenant.authorization_epoch,
        runtime_epoch: bundle.tenant.runtime_epoch,
        version: version_snapshot(crate::ProductPlanVersionBundle {
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
