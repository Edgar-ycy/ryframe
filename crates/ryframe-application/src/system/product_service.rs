use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use chrono::{DateTime, Duration, Utc};
use ryframe_db::{
    ControlDatabaseCluster, ProductPlanVersionBundle, ProductRepository,
    TenantOperationLeaseRepository,
    entities::{
        product_plan, product_plan_capability, product_plan_version, tenant_capability_override,
        tenant_operation_lease,
    },
};
use ryframe_kernel::{ActorContext, AppError, AppResult};
use sea_orm::TransactionTrait;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    AuthorizationCache, ProductReadPort, ProductVersionSnapshot, TenantCapabilityOverrideRecord,
    TenantProductSnapshot,
};

use super::product_capability_catalog::{
    CAPABILITY_CATALOG, SERVICE_ACCOUNTS_CAPABILITY, project_client_config,
    validate_capability_snapshot,
};

const SYSTEM_TENANT_ID: &str = "system";
const PLAN_STATUS_ENABLED: &str = "1";
const VERSION_DRAFT: &str = "draft";
const VERSION_PUBLISHED: &str = "published";
const VERSION_RETIRED: &str = "retired";
const PRODUCT_CHANGE_LEASE_SECONDS: i64 = 30;

mod context;
mod model;
mod read;
mod resources;
mod support;

pub use model::*;
use support::*;

pub struct ProductService {
    db: ControlDatabaseCluster,
    read: Arc<dyn ProductReadPort>,
    repository: ProductRepository,
    operation_leases: TenantOperationLeaseRepository,
    authorization_cache: AuthorizationCache,
    service_accounts_deployment_available: bool,
}

impl ProductService {
    pub fn new(
        db: ControlDatabaseCluster,
        read: Arc<dyn ProductReadPort>,
        authorization_cache: AuthorizationCache,
        service_accounts_deployment_available: bool,
    ) -> Self {
        Self {
            db,
            read,
            repository: ProductRepository,
            operation_leases: TenantOperationLeaseRepository,
            authorization_cache,
            service_accounts_deployment_available,
        }
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
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        if self
            .repository
            .find_plan_by_key(&transaction, &command.key)
            .await?
            .is_some()
        {
            return Err(AppError::Conflict("产品套餐标识已存在".into()));
        }
        let now = Utc::now();
        let plan_id = crate::next_id()?;
        let plan = self
            .repository
            .insert_plan_in_txn(
                &transaction,
                product_plan::Model {
                    id: plan_id,
                    plan_key: command.key,
                    name: command.name,
                    description: command.description,
                    status: PLAN_STATUS_ENABLED.into(),
                    created_by: actor.user_id,
                    created_at: now,
                    updated_at: now,
                },
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        Ok(ProductPlanVo {
            id: plan.id.to_string(),
            key: plan.plan_key,
            name: plan.name,
            description: plan.description,
            status: plan.status,
            created_by: plan.created_by.to_string(),
            versions: Vec::new(),
        })
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
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let mut plan = self
            .repository
            .lock_plan_by_id_in_txn(&transaction, plan_id)
            .await?;
        plan.name = command.name;
        plan.description = command.description;
        plan.status = command.status;
        plan.updated_at = Utc::now();
        let plan = self
            .repository
            .update_plan_in_txn(&transaction, plan)
            .await?;
        crate::commit_current_audit(transaction).await?;
        let versions = self.versions(actor, plan_id).await?;
        Ok(ProductPlanVo {
            id: plan.id.to_string(),
            key: plan.plan_key,
            name: plan.name,
            description: plan.description,
            status: plan.status,
            created_by: plan.created_by.to_string(),
            versions,
        })
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
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let plan = self
            .repository
            .lock_plan_by_id_in_txn(&transaction, plan_id)
            .await?;
        let number = self
            .repository
            .next_version_in_txn(&transaction, plan.id)
            .await?;
        let now = Utc::now();
        let id = crate::next_id()?;
        let version = self
            .repository
            .insert_version_in_txn(
                &transaction,
                product_plan_version::Model {
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
                capability_models(id, capabilities, now),
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        let capabilities = self
            .repository
            .list_capabilities(self.db.write(), id)
            .await?;
        self.version_vo(version, capabilities)
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
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let plan = self
            .repository
            .lock_plan_by_id_in_txn(&transaction, plan_id)
            .await?;
        let mut version = self
            .repository
            .lock_version_in_txn(&transaction, plan.id, number)
            .await?;
        if version.status != VERSION_DRAFT {
            return Err(AppError::Conflict(
                "已发布或已退役的产品套餐版本不可修改".into(),
            ));
        }
        version.name = command.name;
        version.description = command.description;
        version.updated_at = Utc::now();
        let id = version.id;
        let saved = self
            .repository
            .replace_draft_version_in_txn(
                &transaction,
                version,
                capability_models(id, capabilities, Utc::now()),
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        let capabilities = self
            .repository
            .list_capabilities(self.db.write(), id)
            .await?;
        self.version_vo(saved, capabilities)
    }

    pub async fn publish_version(
        &self,
        actor: &ActorContext,
        plan_id: i64,
        number: i32,
    ) -> AppResult<ProductPlanVersionVo> {
        ensure_platform_actor(actor)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let plan = self
            .repository
            .lock_plan_by_id_in_txn(&transaction, plan_id)
            .await?;
        let mut version = self
            .repository
            .lock_version_in_txn(&transaction, plan.id, number)
            .await?;
        if version.status != VERSION_DRAFT {
            return Err(AppError::Conflict("只有草稿版本可以发布".into()));
        }
        let capabilities = self
            .repository
            .list_capabilities(&transaction, version.id)
            .await?;
        self.validate_publishable_capabilities(&capabilities)?;
        version.status = VERSION_PUBLISHED.into();
        version.published_by = Some(actor.user_id);
        version.published_at = Some(Utc::now());
        version.updated_at = Utc::now();
        let saved = self
            .repository
            .transition_version_status_in_txn(
                &transaction,
                version,
                VERSION_DRAFT,
                VERSION_PUBLISHED,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.version_vo(saved, capabilities)
    }

    pub async fn retire_version(
        &self,
        actor: &ActorContext,
        plan_id: i64,
        number: i32,
    ) -> AppResult<ProductPlanVersionVo> {
        ensure_platform_actor(actor)?;
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let plan = self
            .repository
            .lock_plan_by_id_in_txn(&transaction, plan_id)
            .await?;
        let mut version = self
            .repository
            .lock_version_in_txn(&transaction, plan.id, number)
            .await?;
        if version.status != VERSION_PUBLISHED {
            return Err(AppError::Conflict(
                "只有 published 产品套餐版本可以退役".into(),
            ));
        }
        let capabilities = self
            .repository
            .list_capabilities(&transaction, version.id)
            .await?;
        version.status = VERSION_RETIRED.into();
        version.updated_at = Utc::now();
        let saved = self
            .repository
            .transition_version_status_in_txn(
                &transaction,
                version,
                VERSION_PUBLISHED,
                VERSION_RETIRED,
            )
            .await?;
        crate::commit_current_audit(transaction).await?;
        self.version_vo(saved, capabilities)
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
        if !constant_time_eq(plan_hash.as_bytes(), expected_hash.as_bytes()) {
            return Err(AppError::Conflict(
                "产品变更计划哈希无效，请重新预览".into(),
            ));
        }
        self.published_target(target.plan_version_id).await?;
        let owner_token = Uuid::new_v4().to_string();
        let transaction = self.db.write().begin().await.map_err(database_error)?;
        let locked_tenant = self
            .operation_leases
            .lock_tenant_and_validate_in_txn(&transaction, tenant_id, None)
            .await?;
        if locked_tenant.status != ryframe_db::entities::tenant::Model::STATUS_ENABLED {
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
        let now = self.operation_leases.database_utc_now(&transaction).await?;
        self.operation_leases
            .acquire_in_txn(
                &transaction,
                tenant_operation_lease::Model {
                    tenant_id: tenant_id.to_owned(),
                    owner_token: owner_token.clone(),
                    operation: "product.change".into(),
                    resource_type: "product_plan_version".into(),
                    resource_id: target.plan_version_id.to_string(),
                    expires_at: now + Duration::seconds(PRODUCT_CHANGE_LEASE_SECONDS),
                    created_at: now,
                    updated_at: now,
                },
            )
            .await?;
        // 先做一次非锁定读取取得不可变的 plan_id，再统一按 plan -> version 加锁。
        // 这既避免与套餐停用并发错配，也与发布、退役和租户创建保持同一锁序。
        let observed_target = self
            .repository
            .find_version_by_id(&transaction, target.plan_version_id)
            .await?
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))?;
        let locked_plan = self
            .repository
            .lock_plan_by_id_in_txn(&transaction, observed_target.plan.id)
            .await?;
        let locked_version = self
            .repository
            .lock_version_by_id_in_txn(&transaction, target.plan_version_id)
            .await?;
        if locked_version.plan_id != locked_plan.id {
            return Err(AppError::Conflict(
                "目标产品套餐版本所属套餐已变化，请重新预览".into(),
            ));
        }
        if locked_plan.status != PLAN_STATUS_ENABLED {
            return Err(AppError::Conflict("目标产品套餐已停用".into()));
        }
        if locked_version.status != VERSION_PUBLISHED {
            return Err(AppError::Conflict(
                "目标产品套餐版本已不再是 published，请重新预览".into(),
            ));
        }
        // 在 plan/version 行锁之后重新读取能力快照，不能使用加锁前的 bundle。
        let target_bundle = self
            .repository
            .find_version_by_id(&transaction, target.plan_version_id)
            .await?
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))?;
        let mut assignment = self
            .repository
            .lock_assignment_in_txn(&transaction, tenant_id)
            .await?;
        let current_bundle = self
            .repository
            .tenant_product(&transaction, tenant_id)
            .await?
            .ok_or_else(|| AppError::NotFound("租户不存在".into()))?;
        let current = self.context_from_bundle(current_bundle)?;
        ensure_override_change_allowed(
            &current.overrides,
            &normalized,
            capability_override_allowed,
        )?;
        let target_context =
            self.target_context(tenant_id, &epoch_text, target_bundle, &normalized)?;
        let authorization_changed =
            capability_changes(&current, &target_context)
                .iter()
                .any(|change| {
                    CAPABILITY_CATALOG.iter().any(|descriptor| {
                        descriptor.code == change.capability_code
                            && descriptor.affects_authorization
                    })
                });
        self.sync_capability_resources_in_txn(&transaction, tenant_id, &current, &target_context)
            .await?;
        assignment.plan_version_id = target.plan_version_id;
        assignment.changed_by = Some(actor.user_id);
        assignment.change_reason = reason;
        assignment.updated_at = now;
        self.repository
            .replace_assignment_and_overrides_in_txn(
                &transaction,
                assignment,
                override_models(tenant_id, actor.user_id, normalized, now),
            )
            .await?;
        self.repository
            .increment_runtime_epoch_in_txn(&transaction, tenant_id, preview_runtime_epoch)
            .await?;
        let authorization_epoch = if authorization_changed {
            Some(
                self.authorization_cache
                    .increment_tenant_epoch_in_transaction(&transaction, tenant_id)
                    .await?,
            )
        } else {
            None
        };
        self.operation_leases
            .release_in_txn(&transaction, tenant_id, &owner_token)
            .await?;
        crate::commit_current_audit(transaction).await?;
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

    pub async fn validate_assignable_version(&self, version_id: i64) -> AppResult<()> {
        self.published_target(version_id).await.map(|_| ())
    }

    fn version_vo(
        &self,
        version: product_plan_version::Model,
        capabilities: Vec<product_plan_capability::Model>,
    ) -> AppResult<ProductPlanVersionVo> {
        self.validate_capability_models(&capabilities)?;
        Ok(ProductPlanVersionVo {
            id: version.id.to_string(),
            version: version.version,
            name: version.name,
            description: version.description,
            status: version.status,
            created_by: version.created_by.to_string(),
            published_by: version.published_by.map(|value| value.to_string()),
            published_at: version.published_at,
            capabilities: capabilities
                .into_iter()
                .map(|capability| ProductCapabilityVo {
                    capability_code: capability.capability_code,
                    variant_code: capability.variant_code,
                    schema_version: capability.schema_version,
                    config: capability.config,
                })
                .collect(),
        })
    }

    fn validate_capability_models(
        &self,
        capabilities: &[product_plan_capability::Model],
    ) -> AppResult<()> {
        let mut seen = BTreeSet::new();
        for capability in capabilities {
            if !seen.insert(&capability.capability_code) {
                return Err(AppError::Config(format!(
                    "产品套餐版本重复定义能力 {}",
                    capability.capability_code
                )));
            }
            validate_capability_snapshot(
                &capability.capability_code,
                &capability.variant_code,
                capability.schema_version,
                &capability.config,
            )?;
        }
        Ok(())
    }

    fn validate_publishable_capabilities(
        &self,
        capabilities: &[product_plan_capability::Model],
    ) -> AppResult<()> {
        self.validate_capability_relationships(capabilities)?;
        for capability in capabilities {
            let descriptor = CAPABILITY_CATALOG
                .iter()
                .find(|descriptor| descriptor.code == capability.capability_code)
                .expect("capabilities were validated above");
            if !self.deployment_enabled(descriptor.code) {
                return Err(AppError::CapabilityUnavailable(format!(
                    "当前部署不满足能力 {} 的依赖: {}",
                    descriptor.code,
                    descriptor.deployment_dependencies.join(", ")
                )));
            }
        }
        Ok(())
    }

    fn validate_capability_relationships(
        &self,
        capabilities: &[product_plan_capability::Model],
    ) -> AppResult<()> {
        self.validate_capability_models(capabilities)?;
        let enabled = capabilities
            .iter()
            .map(|value| value.capability_code.as_str())
            .collect::<BTreeSet<_>>();
        for capability in capabilities {
            let descriptor = CAPABILITY_CATALOG
                .iter()
                .find(|descriptor| descriptor.code == capability.capability_code)
                .expect("capabilities were validated above");
            if let Some(dependency) = descriptor
                .dependencies
                .iter()
                .find(|dependency| !enabled.contains(**dependency))
            {
                return Err(AppError::Validation(format!(
                    "能力 {} 缺少依赖 {}",
                    descriptor.code, dependency
                )));
            }
            if let Some(conflict) = descriptor
                .conflicts
                .iter()
                .find(|conflict| enabled.contains(**conflict))
            {
                return Err(AppError::Validation(format!(
                    "能力 {} 与 {} 冲突",
                    descriptor.code, conflict
                )));
            }
        }
        Ok(())
    }

    fn context_from_bundle(
        &self,
        bundle: ryframe_db::TenantProductBundle,
    ) -> AppResult<ProductContextVo> {
        self.context_from_snapshot(crate::legacy_product_persistence::tenant_snapshot(bundle))
    }

    fn context_from_snapshot(
        &self,
        snapshot: TenantProductSnapshot,
    ) -> AppResult<ProductContextVo> {
        if snapshot.version.version_status == VERSION_DRAFT {
            return Err(AppError::Config(
                "租户不能绑定尚未发布的产品套餐版本".into(),
            ));
        }
        self.context(
            &snapshot.tenant_id,
            snapshot.runtime_epoch,
            &snapshot.version,
            &snapshot.overrides,
        )
    }

    fn target_context(
        &self,
        tenant_id: &str,
        runtime_epoch: &str,
        target: ProductPlanVersionBundle,
        overrides: &[CapabilityOverrideInput],
    ) -> AppResult<ProductContextVo> {
        let override_records = overrides
            .iter()
            .map(|value| TenantCapabilityOverrideRecord {
                code: value.capability_code.clone(),
                enabled: value.enabled,
                variant: value.variant_code.clone(),
                schema_version: value.schema_version,
                config: value.config.clone(),
                reason: None,
                changed_by: None,
            })
            .collect::<Vec<_>>();
        let epoch = runtime_epoch
            .parse::<i64>()
            .map_err(|_| AppError::Internal("内部 runtime_epoch 无效".into()))?;
        let target = crate::legacy_product_persistence::version_snapshot(target);
        let context = self.context(tenant_id, epoch, &target, &override_records)?;
        self.validate_target_context(&context)?;
        Ok(context)
    }

    fn context(
        &self,
        tenant_id: &str,
        runtime_epoch: i64,
        target: &ProductVersionSnapshot,
        overrides: &[TenantCapabilityOverrideRecord],
    ) -> AppResult<ProductContextVo> {
        self.validate_capability_record_relationships(&target.capabilities)?;
        let plan_capabilities = target
            .capabilities
            .iter()
            .map(|capability| (capability.code.as_str(), capability))
            .collect::<BTreeMap<_, _>>();
        let mut override_capabilities = BTreeMap::new();
        for value in overrides {
            validate_capability_snapshot(
                &value.code,
                &value.variant,
                value.schema_version,
                &value.config,
            )?;
            if override_capabilities
                .insert(value.code.as_str(), value)
                .is_some()
            {
                return Err(AppError::Config(format!("租户重复覆盖能力 {}", value.code)));
            }
        }
        let mut capabilities = Vec::with_capacity(CAPABILITY_CATALOG.len());
        for descriptor in CAPABILITY_CATALOG {
            let plan_value = plan_capabilities.get(descriptor.code).copied();
            let override_value = override_capabilities.get(descriptor.code).copied();
            let (entitled, source, variant_code, schema_version, config) =
                if let Some(value) = override_value {
                    (
                        value.enabled,
                        "override",
                        Some(value.variant.clone()),
                        Some(value.schema_version),
                        Some(project_client_config(descriptor, &value.config)),
                    )
                } else if let Some(value) = plan_value {
                    (
                        true,
                        "plan",
                        Some(value.variant.clone()),
                        Some(value.schema_version),
                        Some(project_client_config(descriptor, &value.config)),
                    )
                } else {
                    (false, "none", None, None, None)
                };
            let deployment_enabled = self.deployment_enabled(descriptor.code);
            capabilities.push(EffectiveCapabilityVo {
                capability_code: descriptor.code.to_owned(),
                name: descriptor.name.to_owned(),
                enabled: entitled && deployment_enabled,
                entitled,
                deployment_enabled,
                source: source.into(),
                variant_code,
                schema_version,
                config,
            });
        }
        Ok(ProductContextVo {
            tenant_id: tenant_id.to_owned(),
            runtime_epoch: runtime_epoch.to_string(),
            plan_key: target.plan_key.clone(),
            plan_name: target.plan_name.clone(),
            plan_version_id: target.version_id.to_string(),
            plan_version: target.version,
            capabilities,
            overrides: overrides
                .iter()
                .map(|value| CapabilityOverrideVo {
                    capability_code: value.code.clone(),
                    enabled: value.enabled,
                    variant_code: value.variant.clone(),
                    schema_version: value.schema_version,
                    config: value.config.clone(),
                    reason: value.reason.clone(),
                    changed_by: value.changed_by.map(|changed_by| changed_by.to_string()),
                })
                .collect(),
        })
    }

    async fn published_target(&self, version_id: i64) -> AppResult<ProductPlanVersionBundle> {
        let target = self
            .repository
            .find_version_by_id(self.db.write(), version_id)
            .await?
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))?;
        if target.plan.status != PLAN_STATUS_ENABLED {
            return Err(AppError::Conflict("目标产品套餐已停用".into()));
        }
        match target.version.status.as_str() {
            VERSION_PUBLISHED => {}
            VERSION_DRAFT => {
                return Err(AppError::Conflict("草稿产品套餐版本不可分配".into()));
            }
            VERSION_RETIRED => {
                return Err(AppError::Conflict("已退役产品套餐版本不可新分配".into()));
            }
            _ => return Err(AppError::Config("产品套餐版本状态无效".into())),
        }
        self.validate_publishable_capabilities(&target.capabilities)?;
        Ok(target)
    }

    fn validate_target_context(&self, context: &ProductContextVo) -> AppResult<()> {
        let enabled = context
            .capabilities
            .iter()
            .filter(|value| value.entitled)
            .map(|value| value.capability_code.as_str())
            .collect::<BTreeSet<_>>();
        for capability in context.capabilities.iter().filter(|value| value.entitled) {
            let descriptor = CAPABILITY_CATALOG
                .iter()
                .find(|descriptor| descriptor.code == capability.capability_code)
                .ok_or_else(|| {
                    AppError::CapabilityUnavailable(format!(
                        "能力 {} 未编译进当前部署",
                        capability.capability_code
                    ))
                })?;
            if let Some(dependency) = descriptor
                .dependencies
                .iter()
                .find(|dependency| !enabled.contains(**dependency))
            {
                return Err(AppError::Validation(format!(
                    "最终能力集合中 {} 缺少依赖 {}",
                    descriptor.code, dependency
                )));
            }
            if let Some(conflict) = descriptor
                .conflicts
                .iter()
                .find(|conflict| enabled.contains(**conflict))
            {
                return Err(AppError::Validation(format!(
                    "最终能力集合中 {} 与 {} 冲突",
                    descriptor.code, conflict
                )));
            }
            if !capability.deployment_enabled {
                return Err(AppError::CapabilityUnavailable(format!(
                    "当前部署不满足能力 {} 的依赖: {}",
                    descriptor.code,
                    descriptor.deployment_dependencies.join(", ")
                )));
            }
        }
        Ok(())
    }

    fn deployment_enabled(&self, capability_code: &str) -> bool {
        match capability_code {
            SERVICE_ACCOUNTS_CAPABILITY => self.service_accounts_deployment_available,
            _ => false,
        }
    }
}
