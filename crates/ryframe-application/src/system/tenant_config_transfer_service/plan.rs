use super::*;

#[derive(Serialize)]
pub(super) struct PlanHashInput<'a> {
    pub(super) source_package_sha256: &'a str,
    pub(super) target_resources_sha256: String,
    pub(super) target_configuration_version: i64,
    pub(super) target_authorization_epoch: i32,
}

pub(super) struct PreviewPlan {
    pub(super) plan_hash: String,
    pub(super) counts: BTreeMap<String, u64>,
    pub(super) items: Vec<tenant_config_transfer_item::Model>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_preview_plan(
    tenant_id: &str,
    transfer_id: i64,
    source: &ParsedTenantConfigPackage,
    target: &TenantConfigPackageResources,
    allowed_routes: &BTreeMap<String, (String, String)>,
    registered_permissions: &BTreeMap<String, String>,
    configuration_version: i64,
    authorization_epoch: i32,
    calculated_at: DateTime<Utc>,
) -> AppResult<PreviewPlan> {
    let target_canonical = canonical_resources(target)?;
    let plan_hash = sha256_json(&PlanHashInput {
        source_package_sha256: &source.package_sha256,
        target_resources_sha256: sha256_hex(&target_canonical),
        target_configuration_version: configuration_version,
        target_authorization_epoch: authorization_epoch,
    })?;
    let mut descriptions = Vec::<PlanItemDescription>::new();
    compare_resources(
        &source.resources,
        target,
        allowed_routes,
        registered_permissions,
        &mut descriptions,
    )?;
    let mut counts = BTreeMap::new();
    let mut items = Vec::with_capacity(descriptions.len());
    for description in descriptions {
        validate_transfer_item_text(&description.stable_key, 384, "配置稳定键")?;
        validate_transfer_item_text(&description.display_name, 255, "配置显示名称")?;
        *counts.entry(description.action.to_owned()).or_insert(0) += 1;
        items.push(tenant_config_transfer_item::Model {
            id: next_id()?,
            tenant_id: tenant_id.to_owned(),
            transfer_id,
            resource_type: description.resource_type.to_owned(),
            stable_key: description.stable_key,
            display_name: description.display_name,
            action: description.action.to_owned(),
            outcome: tenant_config_transfer_item::Model::OUTCOME_PENDING.to_owned(),
            detail_code: description.detail_code.map(str::to_owned),
            detail: description.detail,
            created_at: calculated_at,
            updated_at: calculated_at,
        });
    }
    Ok(PreviewPlan {
        plan_hash,
        counts,
        items,
    })
}

fn validate_transfer_item_text(value: &str, max_chars: usize, label: &str) -> AppResult<()> {
    if value.chars().count() > max_chars {
        return Err(AppError::Validation(format!(
            "{label}超过数据库字段上限（{max_chars} 个字符）"
        )));
    }
    Ok(())
}

pub(super) struct PlanItemDescription {
    pub(super) resource_type: &'static str,
    pub(super) stable_key: String,
    pub(super) display_name: String,
    pub(super) action: &'static str,
    pub(super) detail_code: Option<&'static str>,
    pub(super) detail: Option<String>,
}

pub(super) fn compare_resources(
    source: &TenantConfigPackageResources,
    target: &TenantConfigPackageResources,
    allowed_routes: &BTreeMap<String, (String, String)>,
    registered_permissions: &BTreeMap<String, String>,
    output: &mut Vec<PlanItemDescription>,
) -> AppResult<()> {
    compare_simple(
        "department",
        source.departments.iter(),
        target.departments.iter(),
        |item| join_path(&item.path),
        |item| item.path.last().cloned().unwrap_or_default(),
        output,
    )?;
    compare_simple(
        "post",
        source.posts.iter(),
        target.posts.iter(),
        |item| item.code.clone(),
        |item| item.name.clone(),
        output,
    )?;
    compare_simple(
        "dict_type",
        source.dict_types.iter(),
        target.dict_types.iter(),
        |item| item.code.clone(),
        |item| item.name.clone(),
        output,
    )?;
    compare_simple(
        "dict_data",
        source.dict_data.iter(),
        target.dict_data.iter(),
        |item| format!("{}:{}:{}", item.type_code.len(), item.type_code, item.value),
        |item| item.label.clone(),
        output,
    )?;
    compare_simple(
        "config",
        source.configs.iter(),
        target.configs.iter(),
        |item| item.key.clone(),
        |item| item.name.clone(),
        output,
    )?;

    let target_permissions = target
        .permissions
        .iter()
        .map(|item| (normalize_stable_key(&item.code), item))
        .collect::<BTreeMap<_, _>>();
    for item in &source.permissions {
        let mut description = simple_description(
            "permission",
            item.code.clone(),
            item.name.clone(),
            target_permissions
                .get(&normalize_stable_key(&item.code))
                .copied(),
            item,
        )?;
        if permission_contains_wildcard(&item.code) || is_platform_only_permission(&item.code) {
            description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
            description.detail_code = Some("protected_permission");
            description.detail = Some("平台专用权限或超级通配权限不能迁移".into());
        } else if item.permission_type == "api" {
            match registered_permissions.get(&normalize_stable_key(&item.code)) {
                None => {
                    description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
                    description.detail_code = Some("permission_not_registered");
                    description.detail = Some("目标环境未注册该接口权限".into());
                }
                Some(canonical_code) if canonical_code != &item.code => {
                    description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
                    description.detail_code = Some("permission_catalog_mismatch");
                    description.detail = Some("接口权限代码大小写与目标注册目录不一致".into());
                }
                Some(_) => {
                    if target_permissions
                        .get(&normalize_stable_key(&item.code))
                        .is_some_and(|target_item| {
                            target_item.permission_type != item.permission_type
                        })
                    {
                        description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
                        description.detail_code = Some("permission_catalog_mismatch");
                        description.detail = Some("目标端注册权限类型与配置包不一致".into());
                    }
                }
            }
        } else if item.permission_type != "api"
            && registered_permissions.contains_key(&normalize_stable_key(&item.code))
        {
            description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
            description.detail_code = Some("permission_catalog_mismatch");
            description.detail = Some("目标端注册的 API 权限不能被配置包改写为菜单权限".into());
        } else if let Some(target_item) = target_permissions.get(&normalize_stable_key(&item.code))
            && target_item.permission_type != item.permission_type
        {
            description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
            description.detail_code = Some("permission_catalog_mismatch");
            description.detail = Some("目标端注册权限类型与配置包不一致".into());
        }
        output.push(description);
    }

    for item in &source.menus {
        let target_item = target.menus.iter().find(|candidate| {
            normalize_stable_key(&candidate.stable_key) == normalize_stable_key(&item.stable_key)
        });
        let mut description = simple_description(
            "menu",
            item.stable_key.clone(),
            item.name.clone(),
            target_item,
            item,
        )?;
        if matches!(item.menu_type.as_str(), "M" | "C") {
            match item.route_key.as_deref().and_then(|route_key| {
                allowed_routes
                    .get(&normalize_stable_key(route_key))
                    .map(|catalog| (route_key, catalog))
            }) {
                None => {
                    description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
                    description.detail_code = Some("route_not_registered");
                    description.detail = Some("目标环境未注册该页面路由".into());
                }
                Some((route_key, (canonical_key, _))) if route_key != canonical_key => {
                    description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
                    description.detail_code = Some("route_catalog_mismatch");
                    description.detail = Some("页面 route_key 大小写与目标注册目录不一致".into());
                }
                Some((_, (_, menu_type))) if menu_type != &item.menu_type => {
                    description.action = tenant_config_transfer_item::Model::ACTION_BLOCKED;
                    description.detail_code = Some("route_catalog_mismatch");
                    description.detail = Some("目标端注册的页面路由类型与配置包不一致".into());
                }
                Some(_) => {}
            }
        }
        output.push(description);
    }
    compare_simple(
        "role",
        source.roles.iter(),
        target.roles.iter(),
        |item| item.code.clone(),
        |item| item.name.clone(),
        output,
    )?;
    Ok(())
}

fn compare_simple<'a, T, I, K, D>(
    resource_type: &'static str,
    source: I,
    target: I,
    key: K,
    display: D,
    output: &mut Vec<PlanItemDescription>,
) -> AppResult<()>
where
    T: Serialize + PartialEq + 'a,
    I: Iterator<Item = &'a T>,
    K: Fn(&T) -> String,
    D: Fn(&T) -> String,
{
    let target = target
        .map(|item| {
            (
                normalize_resource_stable_key(resource_type, &key(item)),
                item,
            )
        })
        .collect::<BTreeMap<_, _>>();
    for item in source {
        let stable_key = key(item);
        output.push(simple_description(
            resource_type,
            stable_key.clone(),
            display(item),
            target
                .get(&normalize_resource_stable_key(resource_type, &stable_key))
                .copied(),
            item,
        )?);
    }
    Ok(())
}

fn simple_description<T: PartialEq>(
    resource_type: &'static str,
    stable_key: String,
    display_name: String,
    target: Option<&T>,
    source: &T,
) -> AppResult<PlanItemDescription> {
    let action = match target {
        None => tenant_config_transfer_item::Model::ACTION_CREATE,
        Some(target) if target == source => tenant_config_transfer_item::Model::ACTION_UNCHANGED,
        Some(_) => tenant_config_transfer_item::Model::ACTION_UPDATE,
    };
    Ok(PlanItemDescription {
        resource_type,
        stable_key,
        display_name,
        action,
        detail_code: None,
        detail: None,
    })
}

pub(super) fn canonical_resources(resources: &TenantConfigPackageResources) -> AppResult<Vec<u8>> {
    let mut resources = resources.clone();
    resources.canonicalize();
    serde_json::to_vec(&resources).map_err(internal_json_error)
}

pub(super) fn sha256_json(value: &impl Serialize) -> AppResult<String> {
    serde_json::to_vec(value)
        .map(|value| sha256_hex(&value))
        .map_err(internal_json_error)
}

pub(super) fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}
