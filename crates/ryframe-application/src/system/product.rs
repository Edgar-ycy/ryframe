use std::{collections::BTreeMap, sync::Arc};

use chrono::{Duration, Utc};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AuthorizationCache,
    ports::product::{
        ProductAssignmentChange, ProductPlanState, ProductReadPort, ProductTransactionPort,
        ProductVersionSnapshot, ProductVersionState, ProductWritePort,
        TenantCapabilityOverrideRecord, TenantProductSnapshot,
    },
};

use super::product_capability_catalog::{
    CAPABILITY_CATALOG, project_client_config, validate_capability_snapshot,
};

const SYSTEM_TENANT_ID: &str = "system";
const PLAN_STATUS_ENABLED: &str = "1";
const VERSION_DRAFT: &str = "draft";
const VERSION_PUBLISHED: &str = "published";
const VERSION_RETIRED: &str = "retired";
const TENANT_STATUS_ENABLED: &str = "enabled";
const PRODUCT_CHANGE_LEASE_SECONDS: i64 = 30;

mod context;
mod model;
mod read;
mod resources;
mod support;
mod validation;

pub use crate::ports::product::ProvisioningCapabilityResources;
pub use model::*;
use support::*;

pub struct ProductService {
    read: Arc<dyn ProductReadPort>,
    write: Arc<dyn ProductWritePort>,
    authorization_cache: AuthorizationCache,
    deployment: ProductDeployment,
}

/// 组合根确认的实际部署能力，不从套餐或权限推断基础设施。
#[derive(Clone, Copy, Default)]
pub struct ProductDeployment {
    pub redis: bool,
    pub messaging: bool,
    pub scheduler: bool,
}

impl ProductService {
    pub fn new(
        read: Arc<dyn ProductReadPort>,
        write: Arc<dyn ProductWritePort>,
        authorization_cache: AuthorizationCache,
    ) -> Self {
        Self {
            read,
            write,
            authorization_cache,
            deployment: ProductDeployment::default(),
        }
    }

    pub fn with_deployment(mut self, deployment: ProductDeployment) -> Self {
        self.deployment = deployment;
        self
    }

    pub fn capability_catalog(&self, actor: &ActorContext) -> AppResult<Vec<CapabilityCatalogVo>> {
        ensure_platform_actor(actor)?;
        Ok(CAPABILITY_CATALOG
            .iter()
            .map(|descriptor| CapabilityCatalogVo {
                code: descriptor.code.to_owned(),
                name: descriptor.name.to_owned(),
                description: descriptor.description.to_owned(),
                affects_authorization: descriptor.affects_authorization,
                dependencies: string_slice(descriptor.dependencies),
                conflicts: string_slice(descriptor.conflicts),
                route_keys: string_slice(descriptor.route_keys),
                permission_codes: string_slice(descriptor.permission_codes),
                default_admin_permissions: string_slice(descriptor.default_admin_permissions),
                deployment_dependencies: string_slice(descriptor.deployment_dependencies),
                client_config_fields: string_slice(descriptor.client_config_fields),
                deployment_available: self.deployment_enabled(descriptor.code),
                variants: descriptor
                    .variants
                    .iter()
                    .map(|variant| CapabilityVariantVo {
                        code: variant.code.to_owned(),
                        schema_version: variant.schema_version,
                    })
                    .collect(),
            })
            .collect())
    }

    pub async fn list_plans(&self, actor: &ActorContext) -> AppResult<Vec<ProductPlanVo>> {
        ensure_platform_actor(actor)?;
        self.read
            .list_plans()
            .await?
            .into_iter()
            .map(Self::plan_record_vo)
            .collect()
    }

    pub async fn versions(
        &self,
        actor: &ActorContext,
        plan_id: i64,
    ) -> AppResult<Vec<ProductPlanVersionVo>> {
        ensure_platform_actor(actor)?;
        let plan = self
            .read
            .find_plan(plan_id)
            .await?
            .ok_or_else(|| AppError::NotFound("产品套餐不存在".into()))?;
        plan.versions
            .into_iter()
            .map(Self::version_record_vo)
            .collect()
    }

    pub async fn plan(&self, actor: &ActorContext, plan_id: i64) -> AppResult<ProductPlanVo> {
        ensure_platform_actor(actor)?;
        let plan = self
            .read
            .find_plan(plan_id)
            .await?
            .ok_or_else(|| AppError::NotFound("产品套餐不存在".into()))?;
        Self::plan_record_vo(plan)
    }

    pub async fn create_plan(
        &self,
        actor: &ActorContext,
        command: CreateProductPlanCommand,
    ) -> AppResult<ProductPlanVo> {
        ensure_platform_actor(actor)?;
        validate_plan_key(&command.key)?;
        validate_name(&command.name, "套餐名称")?;
        let transaction = self.write.begin().await?;
        if transaction.plan_key_exists(&command.key).await? {
            return Err(AppError::Conflict("产品套餐标识已存在".into()));
        }
        let now = Utc::now();
        let plan_id = crate::next_id()?;
        let plan = transaction
            .insert_plan(ProductPlanState {
                id: plan_id,
                key: command.key,
                name: command.name,
                description: command.description,
                status: PLAN_STATUS_ENABLED.into(),
                created_by: actor.user_id,
                created_at: now,
                updated_at: now,
            })
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        Ok(Self::plan_state_vo(plan, Vec::new()))
    }

    pub async fn update_plan(
        &self,
        actor: &ActorContext,
        plan_id: i64,
        command: UpdateProductPlanCommand,
    ) -> AppResult<ProductPlanVo> {
        ensure_platform_actor(actor)?;
        validate_name(&command.name, "套餐名称")?;
        validate_plan_status(&command.status)?;
        let transaction = self.write.begin().await?;
        let mut plan = transaction.lock_plan(plan_id).await?;
        plan.name = command.name;
        plan.description = command.description;
        plan.status = command.status;
        plan.updated_at = Utc::now();
        let plan = transaction.save_plan(plan).await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        let versions = self.versions(actor, plan_id).await?;
        Ok(Self::plan_state_vo(plan, versions))
    }

    pub async fn create_version(
        &self,
        actor: &ActorContext,
        plan_id: i64,
        command: CreateProductPlanVersionCommand,
    ) -> AppResult<ProductPlanVersionVo> {
        ensure_platform_actor(actor)?;
        validate_name(&command.name, "版本名称")?;
        let capabilities = normalize_capability_snapshots(command.capabilities)?;
        let transaction = self.write.begin().await?;
        let plan = transaction.lock_plan(plan_id).await?;
        let number = transaction.next_version(plan.id).await?;
        let now = Utc::now();
        let id = crate::next_id()?;
        let result = transaction
            .insert_version(
                ProductVersionState {
                    id,
                    plan_id: plan.id,
                    version: number,
                    name: command.name,
                    description: command.description,
                    status: VERSION_DRAFT.into(),
                    created_by: actor.user_id,
                    published_by: None,
                    published_at: None,
                    created_at: now,
                    updated_at: now,
                },
                capability_records(capabilities),
                now,
            )
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        Self::version_state_vo(result.version, result.capabilities)
    }

    pub async fn update_version(
        &self,
        actor: &ActorContext,
        plan_id: i64,
        number: i32,
        command: UpdateProductPlanVersionCommand,
    ) -> AppResult<ProductPlanVersionVo> {
        ensure_platform_actor(actor)?;
        validate_name(&command.name, "版本名称")?;
        let capabilities = normalize_capability_snapshots(command.capabilities)?;
        let transaction = self.write.begin().await?;
        let plan = transaction.lock_plan(plan_id).await?;
        let mut version = transaction.lock_version(plan.id, number).await?;
        if version.status != VERSION_DRAFT {
            return Err(AppError::Conflict(
                "已发布或已退役的产品套餐版本不可修改".into(),
            ));
        }
        version.name = command.name;
        version.description = command.description;
        version.updated_at = Utc::now();
        let result = transaction
            .replace_draft_version(version, capability_records(capabilities), Utc::now())
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        Self::version_state_vo(result.version, result.capabilities)
    }

    pub async fn publish_version(
        &self,
        actor: &ActorContext,
        plan_id: i64,
        number: i32,
    ) -> AppResult<ProductPlanVersionVo> {
        ensure_platform_actor(actor)?;
        let transaction = self.write.begin().await?;
        let plan = transaction.lock_plan(plan_id).await?;
        let mut version = transaction.lock_version(plan.id, number).await?;
        if version.status != VERSION_DRAFT {
            return Err(AppError::Conflict("只有草稿版本可以发布".into()));
        }
        let capabilities = transaction.capabilities(version.id).await?;
        self.validate_publishable_capability_records(&capabilities)?;
        version.status = VERSION_PUBLISHED.into();
        version.published_by = Some(actor.user_id);
        version.published_at = Some(Utc::now());
        version.updated_at = Utc::now();
        let saved = transaction
            .transition_version(version, VERSION_DRAFT, VERSION_PUBLISHED)
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        Self::version_state_vo(saved, capabilities)
    }

    pub async fn retire_version(
        &self,
        actor: &ActorContext,
        plan_id: i64,
        number: i32,
    ) -> AppResult<ProductPlanVersionVo> {
        ensure_platform_actor(actor)?;
        let transaction = self.write.begin().await?;
        let plan = transaction.lock_plan(plan_id).await?;
        let mut version = transaction.lock_version(plan.id, number).await?;
        if version.status != VERSION_PUBLISHED {
            return Err(AppError::Conflict(
                "只有 published 产品套餐版本可以退役".into(),
            ));
        }
        let capabilities = transaction.capabilities(version.id).await?;
        version.status = VERSION_RETIRED.into();
        version.updated_at = Utc::now();
        let saved = transaction
            .transition_version(version, VERSION_PUBLISHED, VERSION_RETIRED)
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        Self::version_state_vo(saved, capabilities)
    }

    pub async fn preview_change(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        target: ProductChangeTarget,
        capability_override_allowed: bool,
    ) -> AppResult<ProductChangePreviewVo> {
        ensure_platform_actor(actor)?;
        let current = self.effective_context(tenant_id).await?;
        let normalized = normalize_overrides(target.overrides)?;
        ensure_override_change_allowed(
            &current.overrides,
            &normalized,
            capability_override_allowed,
        )?;
        let target_bundle = self.published_target(target.plan_version_id).await?;
        let target_context = self.target_context(
            tenant_id,
            &current.runtime_epoch,
            target_bundle,
            &normalized,
        )?;
        let plan_hash = product_change_hash(
            tenant_id,
            target.plan_version_id,
            &normalized,
            &current.runtime_epoch,
        )?;
        let diff = product_change_diff(&current, &target_context);
        Ok(ProductChangePreviewVo {
            tenant_id: tenant_id.to_owned(),
            runtime_epoch: current.runtime_epoch.clone(),
            plan_hash,
            capability_additions: diff.capability_additions,
            capability_removals: diff.capability_removals,
            capability_changes: diff.capability_changes,
            menu_additions: diff.menu_additions,
            menu_removals: diff.menu_removals,
            permission_additions: diff.permission_additions,
            permission_removals: diff.permission_removals,
            warnings: diff.warnings,
            current,
            target: target_context,
        })
    }

    pub async fn apply_change(
        &self,
        actor: &ActorContext,
        tenant_id: &str,
        command: ApplyProductChangeCommand,
    ) -> AppResult<ProductContextVo> {
        ensure_platform_actor(actor)?;
        let ApplyProductChangeCommand {
            target,
            preview_runtime_epoch,
            plan_hash,
            reason,
            capability_override_allowed,
        } = command;
        if preview_runtime_epoch < 1 {
            return Err(AppError::Validation("runtime_epoch 必须是正整数".into()));
        }
        if reason
            .as_ref()
            .is_some_and(|value| value.chars().count() > 500)
        {
            return Err(AppError::Validation("变更原因不能超过 500 个字符".into()));
        }
        let normalized = normalize_overrides(target.overrides)?;
        let epoch_text = preview_runtime_epoch.to_string();
        let expected_hash =
            product_change_hash(tenant_id, target.plan_version_id, &normalized, &epoch_text)?;
        if !ryframe_auth::constant_time_eq(plan_hash.as_bytes(), expected_hash.as_bytes()) {
            return Err(AppError::Conflict(
                "产品变更计划哈希无效，请重新预览".into(),
            ));
        }
        self.published_target(target.plan_version_id).await?;
        let owner_token = Uuid::new_v4().to_string();
        let transaction = self.write.begin().await?;
        let locked_tenant = transaction.lock_change_tenant(tenant_id).await?;
        if locked_tenant.status != TENANT_STATUS_ENABLED {
            return Err(AppError::TenantOperationConflict(
                "只有 enabled 租户可以提交产品变更；provisioning 必须由创建 Saga 独占完成".into(),
            ));
        }
        if locked_tenant.runtime_epoch != preview_runtime_epoch {
            return Err(AppError::StaleRuntimeEpoch(
                "租户运行时上下文已变化，请重新预览产品变更".into(),
            ));
        }
        let current_authorization_epoch = locked_tenant.authorization_epoch;
        let now = locked_tenant.database_now;
        transaction
            .acquire_change_lease(
                tenant_id,
                &owner_token,
                target.plan_version_id,
                now,
                now + Duration::seconds(PRODUCT_CHANGE_LEASE_SECONDS),
            )
            .await?;
        let target_snapshot = transaction
            .lock_assignable_version(target.plan_version_id)
            .await?;
        if target_snapshot.plan_status != PLAN_STATUS_ENABLED {
            return Err(AppError::Conflict("目标产品套餐已停用".into()));
        }
        if target_snapshot.version_status != VERSION_PUBLISHED {
            return Err(AppError::Conflict(
                "目标产品套餐版本已不再是 published，请重新预览".into(),
            ));
        }
        self.validate_publishable_capability_records(&target_snapshot.capabilities)?;
        let current =
            self.context_from_snapshot(transaction.current_tenant_product(tenant_id).await?)?;
        ensure_override_change_allowed(
            &current.overrides,
            &normalized,
            capability_override_allowed,
        )?;
        let target_context =
            self.target_context(tenant_id, &epoch_text, target_snapshot, &normalized)?;
        let authorization_changed =
            capability_changes(&current, &target_context)
                .iter()
                .any(|change| {
                    CAPABILITY_CATALOG.iter().any(|descriptor| {
                        descriptor.code == change.capability_code
                            && descriptor.affects_authorization
                    })
                });
        let resources = resources::resources_for_change(&current, &target_context);
        transaction
            .sync_capability_resources(tenant_id, &resources)
            .await?;
        transaction
            .replace_assignment(ProductAssignmentChange {
                tenant_id: tenant_id.to_owned(),
                version_id: target.plan_version_id,
                changed_by: actor.user_id,
                reason,
                overrides: override_records(actor.user_id, normalized),
                changed_at: now,
            })
            .await?;
        transaction
            .increment_runtime_epoch(tenant_id, preview_runtime_epoch)
            .await?;
        let authorization_epoch = if authorization_changed {
            Some(
                self.authorization_cache
                    .increment_tenant_epoch_in_transaction(
                        transaction.authorization_mirror(),
                        tenant_id,
                    )
                    .await?,
            )
        } else {
            None
        };
        transaction
            .release_change_lease(tenant_id, &owner_token)
            .await?;
        transaction
            .commit(crate::TransactionAuditMode::CurrentRequest)
            .await?;
        if let Some(authorization_epoch) = authorization_epoch {
            self.authorization_cache
                .sync_tenant_epoch(tenant_id, authorization_epoch)
                .await?;
        } else {
            self.authorization_cache
                .publish_tenant_context_changed(tenant_id, current_authorization_epoch)
                .await;
        }
        self.effective_context(tenant_id).await
    }
}
