use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
};

use super::{
    AssetRoot, GeneratedAsset, GeneratedCatalog, MANIFEST_PATH, OwnershipManifest, PlanAction,
    PlannedAsset, ResourceAssetPlan, ResourceError, ResourceWorkspace, desired_entries,
    load_manifest, load_manifest_snapshot, manifests_equal, schema, target_path,
    validate_workspace, verify_no_resource_removal, verify_owned_files,
    verify_unselected_resource_sources,
};

/// 计算单个资源本次会新增或更新的资产。
///
/// 聚合入口只包含 ownership 中已经受管的资源与本次资源；未选择且尚未受管的清单
/// 不会因为生成另一个资源而进入正式代码。
pub fn plan_resource_assets(
    catalog: &GeneratedCatalog,
    resource: &str,
    workspace: ResourceWorkspace<'_>,
) -> Result<Vec<GeneratedAsset>, ResourceError> {
    validate_workspace(workspace)?;
    let manifest = load_manifest(&workspace.backend_root.join(MANIFEST_PATH))?;
    verify_owned_files(&manifest, workspace)?;
    verify_unselected_resource_sources(catalog, resource, &manifest)?;
    schema::validate_evolution(catalog, &manifest, workspace)?;
    let assets = selected_assets(catalog, resource, &manifest)?;
    schema::preserve_initial_migrations(assets, &manifest, workspace, false)
}

/// 计算预览需要展示的新增、更新、删除与未变化资产。
pub fn plan_resource_changes(
    catalog: &GeneratedCatalog,
    resource: &str,
    workspace: ResourceWorkspace<'_>,
) -> Result<ResourceAssetPlan, ResourceError> {
    let desired = plan_resource_assets(catalog, resource, workspace)?;
    let (manifest, manifest_bytes) =
        load_manifest_snapshot(&workspace.backend_root.join(MANIFEST_PATH))?;
    let mut effective = desired.clone();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.resource != resource && entry.resource != "__catalog__")
    {
        let target = target_path(workspace, entry.root, &entry.path)?;
        let content = fs::read_to_string(&target).map_err(|error| {
            ResourceError::file(
                &target,
                format!("无法读取其他受管资源：{error}"),
                "从版本控制恢复受管文件后重试",
            )
            .with_resource(&entry.resource)
        })?;
        effective.push(GeneratedAsset {
            resource: entry.resource.clone(),
            root: entry.root,
            path: entry.path.clone(),
            content,
        });
    }
    effective.sort_by(|left, right| left.root.cmp(&right.root).then(left.path.cmp(&right.path)));
    let old_by_path = manifest
        .entries
        .iter()
        .map(|entry| ((entry.root, entry.path.as_str()), entry))
        .collect::<BTreeMap<_, _>>();
    let desired_paths = desired
        .iter()
        .map(|asset| (asset.root, asset.path.clone()))
        .collect::<BTreeSet<_>>();
    let mut assets = Vec::new();

    for asset in desired {
        let target = target_path(workspace, asset.root, &asset.path)?;
        let before = match old_by_path.get(&(asset.root, asset.path.as_str())) {
            Some(_) => fs::read_to_string(&target).map_err(|error| {
                ResourceError::file(
                    &target,
                    format!("无法读取已有生成文件：{error}"),
                    "从版本控制恢复受管文件后重试",
                )
                .with_resource(&asset.resource)
            })?,
            None if target.exists() => {
                return Err(ResourceError::new(
                    "目标文件已经存在，但不属于当前生成器",
                    "移动该文件，或确认内容后由人工删除；生成器不会覆盖未受管文件",
                )
                .with_resource(&asset.resource)
                .with_file(target.to_string_lossy()));
            }
            None => String::new(),
        };
        let action = if !old_by_path.contains_key(&(asset.root, asset.path.as_str())) {
            PlanAction::Create
        } else if before == asset.content {
            PlanAction::Unchanged
        } else {
            PlanAction::Update
        };
        assets.push(PlannedAsset {
            resource: asset.resource,
            root: asset.root,
            path: asset.path,
            action,
            before,
            after: asset.content,
        });
    }
    for entry in manifest.entries.iter().filter(|entry| {
        (entry.resource == resource || entry.resource == "__catalog__")
            && !desired_paths.contains(&(entry.root, entry.path.clone()))
    }) {
        let target = target_path(workspace, entry.root, &entry.path)?;
        let before = fs::read_to_string(&target).map_err(|error| {
            ResourceError::file(
                &target,
                format!("无法读取待删除生成文件：{error}"),
                "从版本控制恢复受管文件后重试；预览不会隐藏删除",
            )
            .with_resource(&entry.resource)
        })?;
        assets.push(PlannedAsset {
            resource: entry.resource.clone(),
            root: entry.root,
            path: entry.path.clone(),
            action: PlanAction::Delete,
            before,
            after: String::new(),
        });
    }
    append_manifest_change(&mut assets, &effective, catalog, &manifest, manifest_bytes)?;
    assets.sort_by(|left, right| left.root.cmp(&right.root).then(left.path.cmp(&right.path)));
    Ok(ResourceAssetPlan { assets })
}

/// 计算整个资源目录的新增、更新、删除与未变化资产，但不写入工作区。
///
/// 与 [`write_resources`](super::write_resources) 使用相同的 ownership、迁移冻结和路径
/// 校验；因此适合在自动化门禁中确认生成结果为零差异。
pub fn plan_all_resource_changes(
    catalog: &GeneratedCatalog,
    workspace: ResourceWorkspace<'_>,
) -> Result<ResourceAssetPlan, ResourceError> {
    validate_workspace(workspace)?;
    let manifest_path = workspace.backend_root.join(MANIFEST_PATH);
    let (manifest, manifest_bytes) = load_manifest_snapshot(&manifest_path)?;
    verify_owned_files(&manifest, workspace)?;
    verify_no_resource_removal(catalog, &manifest)?;
    schema::validate_evolution(catalog, &manifest, workspace)?;
    let desired =
        schema::preserve_initial_migrations(catalog.assets.clone(), &manifest, workspace, true)?;
    let old_by_path = manifest
        .entries
        .iter()
        .map(|entry| ((entry.root, entry.path.as_str()), entry))
        .collect::<BTreeMap<_, _>>();
    let desired_paths = desired
        .iter()
        .map(|asset| (asset.root, asset.path.clone()))
        .collect::<BTreeSet<_>>();
    let mut assets = Vec::new();

    for asset in &desired {
        let target = target_path(workspace, asset.root, &asset.path)?;
        let before = match old_by_path.get(&(asset.root, asset.path.as_str())) {
            Some(_) => fs::read_to_string(&target).map_err(|error| {
                ResourceError::file(
                    &target,
                    format!("无法读取已有生成文件：{error}"),
                    "从版本控制恢复受管文件后重试",
                )
                .with_resource(&asset.resource)
            })?,
            None if target.exists() => {
                return Err(ResourceError::new(
                    "目标文件已经存在，但不属于当前生成器",
                    "移动该文件，或确认内容后由人工删除；生成器不会覆盖未受管文件",
                )
                .with_resource(&asset.resource)
                .with_file(target.to_string_lossy()));
            }
            None => String::new(),
        };
        let action = if !old_by_path.contains_key(&(asset.root, asset.path.as_str())) {
            PlanAction::Create
        } else if before == asset.content {
            PlanAction::Unchanged
        } else {
            PlanAction::Update
        };
        assets.push(PlannedAsset {
            resource: asset.resource.clone(),
            root: asset.root,
            path: asset.path.clone(),
            action,
            before,
            after: asset.content.clone(),
        });
    }

    for entry in manifest
        .entries
        .iter()
        .filter(|entry| !desired_paths.contains(&(entry.root, entry.path.clone())))
    {
        let target = target_path(workspace, entry.root, &entry.path)?;
        let before = fs::read_to_string(&target).map_err(|error| {
            ResourceError::file(
                &target,
                format!("无法读取待删除生成文件：{error}"),
                "从版本控制恢复受管文件后重试；检查不会隐藏删除",
            )
            .with_resource(&entry.resource)
        })?;
        assets.push(PlannedAsset {
            resource: entry.resource.clone(),
            root: entry.root,
            path: entry.path.clone(),
            action: PlanAction::Delete,
            before,
            after: String::new(),
        });
    }

    append_manifest_change(&mut assets, &desired, catalog, &manifest, manifest_bytes)?;

    assets.sort_by(|left, right| left.root.cmp(&right.root).then(left.path.cmp(&right.path)));
    Ok(ResourceAssetPlan { assets })
}

fn append_manifest_change(
    planned: &mut Vec<PlannedAsset>,
    desired_assets: &[GeneratedAsset],
    catalog: &GeneratedCatalog,
    manifest: &OwnershipManifest,
    manifest_bytes: Option<Vec<u8>>,
) -> Result<(), ResourceError> {
    let entries = desired_entries(desired_assets, &catalog.resources);
    if manifests_equal(manifest, &entries) {
        return Ok(());
    }
    let next_manifest = OwnershipManifest {
        format_version: 1,
        generator_version: crate::GENERATOR_VERSION.into(),
        entries,
    };
    let after = toml::to_string_pretty(&next_manifest).map_err(|error| {
        ResourceError::new(
            format!("无法序列化 ownership manifest：{error}"),
            "检查 manifest 数据是否只包含稳定基础类型",
        )
    })?;
    let before = manifest_bytes
        .map(|bytes| String::from_utf8(bytes).expect("manifest UTF-8 已在加载时校验"))
        .unwrap_or_default();
    planned.push(PlannedAsset {
        resource: "__catalog__".into(),
        root: AssetRoot::Backend,
        path: MANIFEST_PATH.into(),
        action: if before.is_empty() {
            PlanAction::Create
        } else {
            PlanAction::Update
        },
        before,
        after,
    });
    Ok(())
}

pub(super) fn selected_assets(
    catalog: &GeneratedCatalog,
    resource: &str,
    manifest: &OwnershipManifest,
) -> Result<Vec<GeneratedAsset>, ResourceError> {
    if !catalog.resources.contains_key(resource) {
        return Err(ResourceError::new(
            format!("资源 `{resource}` 不在当前清单中，不能把缺失清单当作删除指令"),
            format!(
                "当前版本不支持 `cargo xtask generate resource {resource}` 按名退役资源；恢复 catalog/resources/{resource}.toml，退役必须由后续显式 remove 命令保留不可变初始迁移后执行"
            ),
        )
        .with_resource(resource));
    }
    let mut names = manifest
        .entries
        .iter()
        .filter(|entry| entry.resource != "__catalog__")
        .map(|entry| entry.resource.as_str())
        .collect::<BTreeSet<_>>();
    names.insert(resource);
    let resources = names
        .into_iter()
        .map(|name| {
            catalog.resources.get(name).cloned().ok_or_else(|| {
                ResourceError::new(
                    format!("已受管资源 `{name}` 缺少当前清单"),
                    format!("恢复 catalog/resources/{name}.toml；移除资源应使用显式删除流程"),
                )
                .with_resource(name)
                .with_file(MANIFEST_PATH)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let managed = super::super::render_resources(&resources)?;
    Ok(managed
        .assets
        .into_iter()
        .filter(|asset| asset.resource == resource || asset.resource == "__catalog__")
        .collect())
}
