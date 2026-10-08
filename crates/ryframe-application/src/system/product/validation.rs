use std::collections::BTreeSet;

use super::*;

impl ProductService {
    pub async fn validate_assignable_version(&self, version_id: i64) -> AppResult<()> {
        self.published_target(version_id).await.map(|_| ())
    }

    pub(super) fn context_from_snapshot(
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

    pub(super) fn target_context(
        &self,
        tenant_id: &str,
        runtime_epoch: &str,
        target: ProductVersionSnapshot,
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
                if tenant_id == SYSTEM_TENANT_ID {
                    (
                        true,
                        "platform",
                        Some("standard".into()),
                        Some(1),
                        Some(serde_json::json!({})),
                    )
                } else if let Some(value) = override_value {
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

    pub(super) async fn published_target(
        &self,
        version_id: i64,
    ) -> AppResult<ProductVersionSnapshot> {
        let target = self
            .read
            .find_version(version_id)
            .await?
            .ok_or_else(|| AppError::NotFound("目标产品套餐版本不存在".into()))?;
        if target.plan_status != PLAN_STATUS_ENABLED {
            return Err(AppError::Conflict("目标产品套餐已停用".into()));
        }
        match target.version_status.as_str() {
            VERSION_PUBLISHED => {}
            VERSION_DRAFT => {
                return Err(AppError::Conflict("草稿产品套餐版本不可分配".into()));
            }
            VERSION_RETIRED => {
                return Err(AppError::Conflict("已退役产品套餐版本不可新分配".into()));
            }
            _ => return Err(AppError::Config("产品套餐版本状态无效".into())),
        }
        self.validate_publishable_capability_records(&target.capabilities)?;
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

    pub(super) fn deployment_enabled(&self, capability_code: &str) -> bool {
        CAPABILITY_CATALOG
            .iter()
            .find(|descriptor| descriptor.code == capability_code)
            .is_some_and(|descriptor| {
                descriptor
                    .deployment_dependencies
                    .iter()
                    .all(|dependency| match *dependency {
                        "redis" => self.deployment.redis,
                        "messaging" => self.deployment.messaging,
                        "scheduler" => self.deployment.scheduler,
                        _ => false,
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::product::{
        ProductCapabilityRecord, ProductPlanRecord, ProductWriteTransaction,
        TenantCapabilityOverrideRecord,
    };
    use async_trait::async_trait;
    use std::collections::BTreeSet;

    struct Read;
    struct Write;

    #[async_trait]
    impl ProductReadPort for Read {
        async fn list_plans(&self) -> AppResult<Vec<ProductPlanRecord>> {
            unreachable!("纯模型测试不读取数据库")
        }

        async fn find_plan(&self, _: i64) -> AppResult<Option<ProductPlanRecord>> {
            unreachable!("纯模型测试不读取数据库")
        }

        async fn find_version(&self, _: i64) -> AppResult<Option<ProductVersionSnapshot>> {
            unreachable!("纯模型测试不读取数据库")
        }

        async fn tenant_product<'a>(
            &'a self,
            _: &'a str,
        ) -> AppResult<Option<TenantProductSnapshot>> {
            unreachable!("纯模型测试不读取数据库")
        }
    }

    #[async_trait]
    impl ProductWritePort for Write {
        async fn begin(&self) -> AppResult<Box<dyn ProductWriteTransaction>> {
            unreachable!("纯模型测试不写入数据库")
        }
    }

    fn service(messaging: bool) -> ProductService {
        ProductService::new(
            Arc::new(Read),
            Arc::new(Write),
            AuthorizationCache::disabled(),
        )
        .with_deployment(ProductDeployment {
            messaging,
            ..ProductDeployment::default()
        })
    }

    fn capability(code: &str) -> ProductCapabilityRecord {
        ProductCapabilityRecord {
            code: code.into(),
            variant: "standard".into(),
            schema_version: 1,
            config: serde_json::json!({}),
        }
    }

    fn snapshot(tenant_id: &str, codes: &[&str]) -> TenantProductSnapshot {
        TenantProductSnapshot {
            tenant_id: tenant_id.into(),
            authorization_epoch: 4,
            runtime_epoch: 7,
            version: ProductVersionSnapshot {
                plan_key: "basic".into(),
                plan_name: "基础套餐".into(),
                plan_status: PLAN_STATUS_ENABLED.into(),
                version_id: 1,
                version: 1,
                version_status: VERSION_PUBLISHED.into(),
                capabilities: codes.iter().map(|code| capability(code)).collect(),
            },
            overrides: Vec::new(),
        }
    }

    #[test]
    fn catalog_contains_only_optional_content_and_has_strict_empty_schema() {
        assert_eq!(
            CAPABILITY_CATALOG
                .iter()
                .map(|item| item.code)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["system.message", "system.notice"])
        );
        for item in CAPABILITY_CATALOG {
            assert!(item.affects_authorization);
            let variant = &item.variants[0];
            assert_eq!(variant.code, "standard");
            assert_eq!(variant.schema_version, 1);
            assert_eq!(
                (variant.config_schema)(),
                serde_json::json!({
                    "type": "object", "properties": {}, "additionalProperties": false
                })
            );
            assert!((variant.validate)(&serde_json::json!({})).is_ok());
            for invalid in [
                serde_json::json!({"enabled": true}),
                serde_json::json!([]),
                serde_json::Value::Null,
            ] {
                assert!((variant.validate)(&invalid).is_err());
            }
            assert!(
                validate_capability_snapshot(item.code, "unknown", 1, &serde_json::json!({}))
                    .is_err()
            );
            assert!(
                validate_capability_snapshot(item.code, "standard", 2, &serde_json::json!({}))
                    .is_err()
            );
        }
    }

    #[test]
    fn empty_plan_keeps_base_permissions_and_only_filters_optional_resources() {
        let service = service(true);
        let base = BTreeSet::from([
            "system:user:list".into(),
            "system:role:edit".into(),
            "system:post:export".into(),
            "monitor:schedule:run".into(),
            "system:user-import:add".into(),
            "monitor:job:list".into(),
        ]);
        let mut requested = base.clone();
        requested.extend(["system:notice:list".into(), "system:message:publish".into()]);
        assert_eq!(
            service
                .filter_syncable_permission_codes(snapshot("a", &[]), requested)
                .unwrap(),
            base
        );
        assert!(
            service
                .ensure_permission_codes_enabled(
                    snapshot("a", &[]),
                    &base.into_iter().collect::<Vec<_>>()
                )
                .is_ok()
        );
        assert!(service.validate_publishable_capability_records(&[]).is_ok());
        let context = SessionProductContextVo {
            authorization_epoch: 4,
            runtime_epoch: "7".into(),
            capabilities: Vec::new(),
        };
        assert_eq!(
            service.disabled_session_route_keys(&context),
            ["system.notice"]
        );
        assert!(
            !service
                .disabled_session_permission_codes(&context)
                .contains("system:post:list")
        );
    }

    #[test]
    fn deployment_and_tenant_entitlement_are_independent_and_fail_closed() {
        let service = service(false);
        assert!(
            service
                .require_capability_snapshot(snapshot("a", &["system.notice"]), "system.notice")
                .is_ok()
        );
        assert!(matches!(
            service.require_capability_snapshot(snapshot("a", &[]), "system.notice"),
            Err(AppError::TenantCapabilityDenied(_))
        ));
        assert!(matches!(
            service
                .require_capability_snapshot(snapshot("a", &["system.message"]), "system.message"),
            Err(AppError::CapabilityUnavailable(_))
        ));
        assert!(
            service
                .validate_publishable_capability_records(&[capability("system.message")])
                .is_err()
        );
        assert!(
            service
                .validate_publishable_capability_records(&[capability("system.notice")])
                .is_ok()
        );
        for stale in ["system.post", "system.user", "monitor.jobs", "unknown"] {
            assert!(
                service
                    .context_from_snapshot(snapshot("a", &[stale]))
                    .is_err()
            );
        }
    }

    #[test]
    fn tenant_overrides_and_changes_manage_only_optional_resources() {
        let service = service(true);
        let current = service
            .context_from_snapshot(snapshot("a", &["system.notice", "system.message"]))
            .unwrap();
        let mut disabled = snapshot("a", &["system.notice", "system.message"]);
        disabled.overrides.push(TenantCapabilityOverrideRecord {
            code: "system.notice".into(),
            enabled: false,
            variant: "standard".into(),
            schema_version: 1,
            config: serde_json::json!({}),
            reason: None,
            changed_by: None,
        });
        assert!(matches!(
            service.require_capability_snapshot(disabled, "system.notice"),
            Err(AppError::TenantCapabilityDenied(_))
        ));
        let target = service.context_from_snapshot(snapshot("a", &[])).unwrap();
        let removed = resources::resources_for_change(&current, &target);
        assert!(removed.enabled_route_keys.is_empty());
        assert_eq!(removed.managed_route_keys, ["system.notice"]);
        assert!(
            !removed
                .managed_permission_codes
                .iter()
                .any(|code| code.starts_with("system:user:"))
        );
        let restored = resources::resources_for_change(&target, &current);
        assert_eq!(restored.enabled_route_keys, ["system.notice"]);
        assert!(
            restored
                .default_admin_permissions
                .contains(&"system:message:publish".into())
        );
        assert!(
            service
                .require_capability_snapshot(snapshot("b", &[]), "system.notice")
                .is_err()
        );
        assert!(
            service
                .require_capability_snapshot(snapshot("a", &["system.notice"]), "system.notice")
                .is_ok()
        );
    }
}
