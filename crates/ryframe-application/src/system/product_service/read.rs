use std::collections::BTreeSet;

use crate::{ProductPlanRecord, ProductVersionRecord};

use super::*;

impl ProductService {
    pub(super) fn plan_record_vo(plan: ProductPlanRecord) -> AppResult<ProductPlanVo> {
        Ok(ProductPlanVo {
            id: plan.id.to_string(),
            key: plan.key,
            name: plan.name,
            description: plan.description,
            status: plan.status,
            created_by: plan.created_by.to_string(),
            versions: plan
                .versions
                .into_iter()
                .map(Self::version_record_vo)
                .collect::<AppResult<Vec<_>>>()?,
        })
    }

    pub(super) fn version_record_vo(
        version: ProductVersionRecord,
    ) -> AppResult<ProductPlanVersionVo> {
        let mut seen = BTreeSet::new();
        for capability in &version.capabilities {
            if !seen.insert(&capability.code) {
                return Err(AppError::Config(format!(
                    "产品套餐版本重复定义能力 {}",
                    capability.code
                )));
            }
            validate_capability_snapshot(
                &capability.code,
                &capability.variant,
                capability.schema_version,
                &capability.config,
            )?;
        }
        Ok(ProductPlanVersionVo {
            id: version.id.to_string(),
            version: version.version,
            name: version.name,
            description: version.description,
            status: version.status,
            created_by: version.created_by.to_string(),
            published_by: version.published_by.map(|value| value.to_string()),
            published_at: version.published_at,
            capabilities: version
                .capabilities
                .into_iter()
                .map(|capability| ProductCapabilityVo {
                    capability_code: capability.code,
                    variant_code: capability.variant,
                    schema_version: capability.schema_version,
                    config: capability.config,
                })
                .collect(),
        })
    }

    pub(super) fn validate_capability_record_relationships(
        &self,
        capabilities: &[crate::ProductCapabilityRecord],
    ) -> AppResult<()> {
        let mut enabled = BTreeSet::new();
        for capability in capabilities {
            if !enabled.insert(capability.code.as_str()) {
                return Err(AppError::Config(format!(
                    "产品套餐版本重复定义能力 {}",
                    capability.code
                )));
            }
            validate_capability_snapshot(
                &capability.code,
                &capability.variant,
                capability.schema_version,
                &capability.config,
            )?;
        }
        for capability in capabilities {
            let descriptor = CAPABILITY_CATALOG
                .iter()
                .find(|descriptor| descriptor.code == capability.code)
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

    pub(super) fn validate_publishable_capability_records(
        &self,
        capabilities: &[crate::ProductCapabilityRecord],
    ) -> AppResult<()> {
        self.validate_capability_record_relationships(capabilities)?;
        for capability in capabilities {
            let descriptor = CAPABILITY_CATALOG
                .iter()
                .find(|descriptor| descriptor.code == capability.code)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability() -> crate::ProductCapabilityRecord {
        crate::ProductCapabilityRecord {
            code: SERVICE_ACCOUNTS_CAPABILITY.into(),
            variant: "default".into(),
            schema_version: 1,
            config: serde_json::json!({}),
        }
    }

    fn version(capabilities: Vec<crate::ProductCapabilityRecord>) -> ProductVersionRecord {
        ProductVersionRecord {
            id: 1,
            version: 1,
            name: "基础版".into(),
            description: None,
            status: VERSION_DRAFT.into(),
            created_by: 2,
            published_by: None,
            published_at: None,
            capabilities,
        }
    }

    #[test]
    fn read_record_rejects_duplicate_capabilities() {
        assert!(ProductService::version_record_vo(version(vec![capability()])).is_ok());
        assert!(
            ProductService::version_record_vo(version(vec![capability(), capability()])).is_err()
        );
    }
}
