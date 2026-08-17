use super::*;

pub(super) fn normalize_capability_snapshots(
    values: Vec<CapabilitySnapshotInput>,
) -> AppResult<Vec<CapabilitySnapshotInput>> {
    let mut by_code = BTreeMap::new();
    for value in values {
        validate_capability_snapshot(
            &value.capability_code,
            &value.variant_code,
            value.schema_version,
            &value.config,
        )?;
        if by_code
            .insert(value.capability_code.clone(), value)
            .is_some()
        {
            return Err(AppError::Validation("能力代码不能重复".into()));
        }
    }
    Ok(by_code.into_values().collect())
}

pub(super) fn normalize_overrides(
    values: Vec<CapabilityOverrideInput>,
) -> AppResult<Vec<CapabilityOverrideInput>> {
    let mut by_code = BTreeMap::new();
    for value in values {
        validate_capability_snapshot(
            &value.capability_code,
            &value.variant_code,
            value.schema_version,
            &value.config,
        )?;
        if by_code
            .insert(value.capability_code.clone(), value)
            .is_some()
        {
            return Err(AppError::Validation("租户能力覆盖代码不能重复".into()));
        }
    }
    Ok(by_code.into_values().collect())
}

pub(super) fn ensure_override_change_allowed(
    current: &[CapabilityOverrideVo],
    target: &[CapabilityOverrideInput],
    allowed: bool,
) -> AppResult<()> {
    let unchanged = current.len() == target.len()
        && target.iter().all(|candidate| {
            current.iter().any(|existing| {
                existing.capability_code == candidate.capability_code
                    && existing.enabled == candidate.enabled
                    && existing.variant_code == candidate.variant_code
                    && existing.schema_version == candidate.schema_version
                    && existing.config == candidate.config
            })
        });
    if unchanged || allowed {
        Ok(())
    } else {
        Err(AppError::PermissionDenied(
            "修改租户能力覆盖需要 tenant:capability:override 权限".into(),
        ))
    }
}

pub(super) fn capability_models(
    version_id: i64,
    values: Vec<CapabilitySnapshotInput>,
    now: DateTime<Utc>,
) -> Vec<product_plan_capability::Model> {
    values
        .into_iter()
        .map(|value| product_plan_capability::Model {
            plan_version_id: version_id,
            capability_code: value.capability_code,
            variant_code: value.variant_code,
            schema_version: value.schema_version,
            config: value.config,
            created_at: now,
            updated_at: now,
        })
        .collect()
}

pub(super) fn override_models(
    tenant_id: &str,
    actor_id: i64,
    values: Vec<CapabilityOverrideInput>,
    now: DateTime<Utc>,
) -> Vec<tenant_capability_override::Model> {
    values
        .into_iter()
        .map(|value| tenant_capability_override::Model {
            tenant_id: tenant_id.to_owned(),
            capability_code: value.capability_code,
            enabled: value.enabled,
            variant_code: value.variant_code,
            schema_version: value.schema_version,
            config: value.config,
            reason: Some("platform_override".into()),
            changed_by: Some(actor_id),
            created_at: now,
            updated_at: now,
        })
        .collect()
}

pub(super) fn product_change_hash(
    tenant_id: &str,
    plan_version_id: i64,
    overrides: &[CapabilityOverrideInput],
    runtime_epoch: &str,
) -> AppResult<String> {
    let canonical = serde_json::json!({
        "tenant_id": tenant_id,
        "plan_version_id": plan_version_id.to_string(),
        "runtime_epoch": runtime_epoch,
        "overrides": overrides.iter().map(|value| serde_json::json!({
            "capability_code": value.capability_code,
            "enabled": value.enabled,
            "variant_code": value.variant_code,
            "schema_version": value.schema_version,
            "config": value.config,
        })).collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| AppError::Internal(format!("产品变更计划序列化失败: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(super) fn capability_changes(
    before: &ProductContextVo,
    after: &ProductContextVo,
) -> Vec<ProductCapabilityChangeVo> {
    before
        .capabilities
        .iter()
        .zip(after.capabilities.iter())
        .filter(|(before, after)| before != after)
        .map(|(before, after)| ProductCapabilityChangeVo {
            capability_code: before.capability_code.clone(),
            before: before.clone(),
            after: after.clone(),
        })
        .collect()
}

pub(super) fn product_change_diff(
    before: &ProductContextVo,
    after: &ProductContextVo,
) -> ProductChangeDiff {
    let before_enabled = before
        .capabilities
        .iter()
        .filter(|value| value.enabled)
        .map(|value| value.capability_code.as_str())
        .collect::<BTreeSet<_>>();
    let after_enabled = after
        .capabilities
        .iter()
        .filter(|value| value.enabled)
        .map(|value| value.capability_code.as_str())
        .collect::<BTreeSet<_>>();
    let capability_additions = after_enabled
        .difference(&before_enabled)
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    let capability_removals = before_enabled
        .difference(&after_enabled)
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    let capability_changes = capability_changes(before, after)
        .into_iter()
        .filter(|change| change.before.enabled && change.after.enabled)
        .collect::<Vec<_>>();
    let before_routes = descriptor_values(&before_enabled, |descriptor| descriptor.route_keys);
    let after_routes = descriptor_values(&after_enabled, |descriptor| descriptor.route_keys);
    let before_permissions =
        descriptor_values(&before_enabled, |descriptor| descriptor.permission_codes);
    let after_permissions =
        descriptor_values(&after_enabled, |descriptor| descriptor.permission_codes);
    let menu_additions = after_routes.difference(&before_routes).cloned().collect();
    let menu_removals = before_routes.difference(&after_routes).cloned().collect();
    let permission_additions = after_permissions
        .difference(&before_permissions)
        .cloned()
        .collect();
    let permission_removals = before_permissions
        .difference(&after_permissions)
        .cloned()
        .collect();
    let mut warnings = Vec::new();
    if before.plan_version_id != after.plan_version_id {
        warnings.push("product_plan_version_will_change".into());
    }
    if !capability_additions.is_empty()
        || !capability_removals.is_empty()
        || !capability_changes.is_empty()
    {
        warnings.push("authorization_and_navigation_will_refresh".into());
    }
    ProductChangeDiff {
        capability_additions,
        capability_removals,
        capability_changes,
        menu_additions,
        menu_removals,
        permission_additions,
        permission_removals,
        warnings,
    }
}

fn descriptor_values(
    enabled: &BTreeSet<&str>,
    values: impl Fn(
        &super::super::product_capability_catalog::CapabilityDescriptor,
    ) -> &'static [&'static str],
) -> BTreeSet<String> {
    CAPABILITY_CATALOG
        .iter()
        .filter(|descriptor| enabled.contains(descriptor.code))
        .flat_map(values)
        .map(|value| (*value).to_owned())
        .collect()
}

pub(super) fn validate_plan_key(value: &str) -> AppResult<()> {
    let valid = (2..=64).contains(&value.len())
        && value.bytes().enumerate().all(|(index, byte)| match byte {
            b'a'..=b'z' => true,
            b'0'..=b'9' | b'_' | b'-' | b'.' => index > 0,
            _ => false,
        });
    if !valid {
        return Err(AppError::Validation(
            "套餐标识必须以小写字母开头，且仅包含小写字母、数字、点、横线或下划线".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_name(value: &str, field: &str) -> AppResult<()> {
    let length = value.trim().chars().count();
    if !(1..=128).contains(&length) {
        return Err(AppError::Validation(format!(
            "{field}长度必须在 1 到 128 个字符之间"
        )));
    }
    Ok(())
}

pub(super) fn validate_plan_status(value: &str) -> AppResult<()> {
    if matches!(value, "0" | "1") {
        Ok(())
    } else {
        Err(AppError::Validation("套餐状态只能是 0 或 1".into()))
    }
}

pub(super) fn ensure_platform_actor(actor: &ActorContext) -> AppResult<()> {
    crate::validated_tenant_id(actor)?;
    if actor.tenant_id != SYSTEM_TENANT_ID {
        return Err(AppError::Authorization(
            "仅系统租户可以访问产品控制面".into(),
        ));
    }
    Ok(())
}

pub(super) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

pub(super) fn database_error(error: impl std::fmt::Display) -> AppError {
    AppError::Database(error.to_string())
}

pub(super) fn string_slice(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
