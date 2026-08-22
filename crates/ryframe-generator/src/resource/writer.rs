use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AssetRoot, GeneratedAsset, GeneratedCatalog, ResourceError};

mod schema;
mod transaction;

use transaction::{
    ExpectedFile, InstalledFile, install_staged, move_to_backup, persist_recovery_directories,
    rollback, verify_expected_file, write_staged,
};

const MANIFEST_PATH: &str = "catalog/resources/.ownership.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipManifest {
    pub format_version: u16,
    pub generator_version: String,
    #[serde(default)]
    pub entries: Vec<OwnershipEntry>,
}

impl Default for OwnershipManifest {
    fn default() -> Self {
        Self {
            format_version: 1,
            generator_version: crate::GENERATOR_VERSION.into(),
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipEntry {
    pub resource: String,
    pub root: AssetRoot,
    pub path: String,
    pub source_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_revision: Option<String>,
    pub content_hash: String,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceWorkspace<'a> {
    pub backend_root: &'a Path,
    pub frontend_root: Option<&'a Path>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SafeWriteReport {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub written: Vec<String>,
    pub removed: Vec<String>,
    pub unchanged: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAction {
    Create,
    Update,
    Delete,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedAsset {
    pub resource: String,
    pub root: AssetRoot,
    pub path: String,
    pub action: PlanAction,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceAssetPlan {
    pub assets: Vec<PlannedAsset>,
}

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
    let manifest = load_manifest(&workspace.backend_root.join(MANIFEST_PATH))?;
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
    assets.sort_by(|left, right| left.root.cmp(&right.root).then(left.path.cmp(&right.path)));
    Ok(ResourceAssetPlan { assets })
}

/// 只写入命名资源及其必要聚合，并保留其他已受管资源。
pub fn write_resource(
    catalog: &GeneratedCatalog,
    resource: &str,
    workspace: ResourceWorkspace<'_>,
) -> Result<SafeWriteReport, ResourceError> {
    validate_workspace(workspace)?;
    let manifest = load_manifest(&workspace.backend_root.join(MANIFEST_PATH))?;
    verify_owned_files(&manifest, workspace)?;
    verify_unselected_resource_sources(catalog, resource, &manifest)?;
    schema::validate_evolution(catalog, &manifest, workspace)?;
    let mut assets = selected_assets(catalog, resource, &manifest)?;

    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.resource != resource && entry.resource != "__catalog__")
    {
        let path = target_path(workspace, entry.root, &entry.path)?;
        let content = fs::read_to_string(&path).map_err(|error| {
            ResourceError::new(
                format!("无法保留其他受管资源：{error}"),
                "从版本控制恢复受管文件后重试",
            )
            .with_resource(&entry.resource)
            .with_file(path.to_string_lossy())
        })?;
        assets.push(GeneratedAsset {
            resource: entry.resource.clone(),
            root: entry.root,
            path: entry.path.clone(),
            content,
        });
    }
    assets.sort_by(|left, right| left.root.cmp(&right.root).then(left.path.cmp(&right.path)));
    let effective = GeneratedCatalog {
        assets,
        explanations: catalog.explanations.clone(),
        resources: catalog.resources.clone(),
    };
    write_resources(&effective, workspace)
}

fn selected_assets(
    catalog: &GeneratedCatalog,
    resource: &str,
    manifest: &OwnershipManifest,
) -> Result<Vec<GeneratedAsset>, ResourceError> {
    if !catalog.resources.contains_key(resource) {
        return Err(ResourceError::new(
            format!("资源 `{resource}` 不在当前清单中，不能把缺失清单当作删除指令"),
            format!(
                "当前版本不支持 `cargo resource {resource}` 按名退役资源；恢复 catalog/resources/{resource}.toml，退役必须由后续显式 remove 命令保留不可变初始迁移后执行"
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
    let managed = super::render_resources(&resources)?;
    Ok(managed
        .assets
        .into_iter()
        .filter(|asset| asset.resource == resource || asset.resource == "__catalog__")
        .collect())
}

/// 安全写入生成目录。
///
/// 所有内容先写入同卷临时目录并核验哈希。受管旧文件的哈希不一致时立即停止；真正
/// 替换使用 rename，任一替换失败都会恢复已备份文件。
pub fn write_resources(
    catalog: &GeneratedCatalog,
    workspace: ResourceWorkspace<'_>,
) -> Result<SafeWriteReport, ResourceError> {
    validate_workspace(workspace)?;
    let manifest_path = workspace.backend_root.join(MANIFEST_PATH);
    let (old_manifest, old_manifest_bytes) = load_manifest_snapshot(&manifest_path)?;
    verify_owned_files(&old_manifest, workspace)?;
    verify_no_resource_removal(catalog, &old_manifest)?;

    schema::validate_evolution(catalog, &old_manifest, workspace)?;
    let desired_assets = schema::preserve_initial_migrations(
        catalog.assets.clone(),
        &old_manifest,
        workspace,
        true,
    )?;
    let desired_entries = desired_entries(&desired_assets, &catalog.resources);
    let old_by_path = old_manifest
        .entries
        .iter()
        .map(|entry| ((entry.root, entry.path.clone()), entry))
        .collect::<BTreeMap<_, _>>();
    let desired_by_path = desired_assets
        .iter()
        .map(|asset| ((asset.root, asset.path.clone()), asset))
        .collect::<BTreeMap<_, _>>();

    let mut report = SafeWriteReport::default();
    let mut changed = Vec::new();
    let mut created_paths = BTreeSet::new();
    for (key, asset) in &desired_by_path {
        let target = target_path(workspace, key.0, &key.1)?;
        let hash = content_hash(asset.content.as_bytes());
        match old_by_path.get(key) {
            Some(old) if old.content_hash == hash => {
                report.unchanged.push(display_path(key.0, &key.1));
            }
            Some(_) => changed.push(*asset),
            None if target.exists() => {
                return Err(ResourceError::new(
                    "目标文件已经存在，但不属于当前生成器",
                    "移动该文件，或确认内容后由人工删除；生成器不会覆盖未受管文件",
                )
                .with_resource(&asset.resource)
                .with_file(target.to_string_lossy()));
            }
            None => {
                created_paths.insert((key.0, key.1.clone()));
                changed.push(*asset);
            }
        }
    }

    let obsolete = old_manifest
        .entries
        .iter()
        .filter(|entry| !desired_by_path.contains_key(&(entry.root, entry.path.clone())))
        .collect::<Vec<_>>();
    let affected = changed
        .iter()
        .map(|asset| (asset.root, asset.path.clone()))
        .chain(
            obsolete
                .iter()
                .map(|entry| (entry.root, entry.path.clone())),
        )
        .collect::<BTreeSet<_>>();
    let expected_files = affected
        .iter()
        .map(|key| {
            let expected = old_by_path.get(key).map_or(ExpectedFile::Absent, |entry| {
                ExpectedFile::ContentHash(entry.content_hash.clone())
            });
            (key.clone(), expected)
        })
        .collect::<BTreeMap<_, _>>();
    let expected_manifest =
        old_manifest_bytes.map_or(ExpectedFile::Absent, ExpectedFile::ExactBytes);
    if changed.is_empty() && obsolete.is_empty() && manifests_equal(&old_manifest, &desired_entries)
    {
        report.unchanged.sort();
        return Ok(report);
    }

    let backend_stage = tempfile::Builder::new()
        .prefix(".ryframe-generator-stage-")
        .tempdir_in(workspace.backend_root)
        .map_err(|error| {
            ResourceError::file(
                workspace.backend_root,
                format!("无法创建后端临时生成目录：{error}"),
                "确认工作区可写且磁盘空间充足",
            )
        })?;
    let touches_frontend = changed
        .iter()
        .any(|asset| asset.root == AssetRoot::Frontend)
        || obsolete
            .iter()
            .any(|entry| entry.root == AssetRoot::Frontend);
    let frontend_stage = if touches_frontend {
        let root = workspace.frontend_root.ok_or_else(|| {
            ResourceError::new(
                "生成结果包含前端资产，但没有提供前端工作区",
                "为 ResourceWorkspace.frontend_root 提供前端仓库路径",
            )
        })?;
        Some(
            tempfile::Builder::new()
                .prefix(".ryframe-generator-stage-")
                .tempdir_in(root)
                .map_err(|error| {
                    ResourceError::file(
                        root,
                        format!("无法创建前端临时生成目录：{error}"),
                        "确认前端工作区可写且磁盘空间充足",
                    )
                })?,
        )
    } else {
        None
    };

    for asset in &changed {
        let stage_root = match asset.root {
            AssetRoot::Backend => backend_stage.path(),
            AssetRoot::Frontend => frontend_stage
                .as_ref()
                .expect("前端资产已创建对应临时目录")
                .path(),
        };
        let staged = stage_root.join(&asset.path);
        write_staged(&staged, &asset.content, &asset.resource)?;
    }

    let new_manifest = OwnershipManifest {
        format_version: 1,
        generator_version: crate::GENERATOR_VERSION.into(),
        entries: desired_entries,
    };
    let manifest_content = toml::to_string_pretty(&new_manifest).map_err(|error| {
        ResourceError::new(
            format!("无法序列化 ownership manifest：{error}"),
            "检查 manifest 数据是否只包含稳定基础类型",
        )
    })?;
    let staged_manifest = backend_stage.path().join(MANIFEST_PATH);
    write_staged(&staged_manifest, &manifest_content, "__catalog__")?;

    let backend_backup = backend_stage.path().join(".backup");
    fs::create_dir_all(&backend_backup).map_err(|error| {
        ResourceError::file(
            &backend_backup,
            format!("无法创建回滚目录：{error}"),
            "确认工作区可写且未被安全软件锁定",
        )
    })?;
    if let Some(stage) = &frontend_stage {
        let frontend_backup = stage.path().join(".backup");
        fs::create_dir_all(&frontend_backup).map_err(|error| {
            ResourceError::file(
                &frontend_backup,
                format!("无法创建前端回滚目录：{error}"),
                "确认前端工作区可写且未被安全软件锁定",
            )
        })?;
    }

    let mut backups: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut installed: Vec<InstalledFile> = Vec::new();
    let transaction = (|| -> Result<(), ResourceError> {
        for ((root, path), expected) in &expected_files {
            let target = target_path(workspace, *root, path)?;
            verify_expected_file(&target, expected)?;
            if expected.exists() {
                let backup = match root {
                    AssetRoot::Backend => backend_backup.join(path),
                    AssetRoot::Frontend => frontend_stage
                        .as_ref()
                        .expect("前端事务必须有同卷临时目录")
                        .path()
                        .join(".backup")
                        .join(path),
                };
                move_to_backup(&target, &backup)?;
                backups.push((target, backup));
            }
        }
        verify_expected_file(&manifest_path, &expected_manifest)?;
        if expected_manifest.exists() {
            let backup = backend_backup.join("ownership.toml");
            move_to_backup(&manifest_path, &backup)?;
            backups.push((manifest_path.clone(), backup));
        }

        for asset in &changed {
            let stage_root = match asset.root {
                AssetRoot::Backend => backend_stage.path(),
                AssetRoot::Frontend => frontend_stage
                    .as_ref()
                    .expect("前端临时目录必须存在")
                    .path(),
            };
            let staged = stage_root.join(&asset.path);
            let target = target_path(workspace, asset.root, &asset.path)?;
            verify_expected_file(&target, &ExpectedFile::Absent)?;
            install_staged(&staged, &target, &asset.resource)?;
            installed.push(InstalledFile {
                path: target,
                content_hash: content_hash(asset.content.as_bytes()),
            });
            let display = display_path(asset.root, &asset.path);
            if created_paths.contains(&(asset.root, asset.path.clone())) {
                report.created.push(display.clone());
            } else {
                report.updated.push(display.clone());
            }
            report.written.push(display);
        }
        for entry in old_manifest
            .entries
            .iter()
            .filter(|entry| !expected_files.contains_key(&(entry.root, entry.path.clone())))
        {
            let target = target_path(workspace, entry.root, &entry.path)?;
            verify_expected_file(
                &target,
                &ExpectedFile::ContentHash(entry.content_hash.clone()),
            )?;
        }
        verify_expected_file(&manifest_path, &ExpectedFile::Absent)?;
        install_staged(&staged_manifest, &manifest_path, "__catalog__")?;
        installed.push(InstalledFile {
            path: manifest_path.clone(),
            content_hash: content_hash(manifest_content.as_bytes()),
        });
        Ok(())
    })();

    if let Err(error) = transaction {
        return match rollback(&installed, &backups) {
            Ok(()) => Err(error),
            Err(rollback_error) => {
                let recovery_directories =
                    persist_recovery_directories(backend_stage, frontend_stage);
                let recovery_paths = recovery_directories
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("；");
                Err(ResourceError::new(
                    format!("写入事务失败：{error}；回滚也未完整完成：{rollback_error}"),
                    format!(
                        "停止继续生成；持久化备份目录为 {recovery_paths}；按错误中的目标与备份路径人工恢复，恢复前不要删除这些目录或 ownership manifest"
                    ),
                ))
            }
        };
    }

    for entry in obsolete {
        report.removed.push(display_path(entry.root, &entry.path));
    }
    report.written.sort();
    report.created.sort();
    report.updated.sort();
    report.removed.sort();
    report.unchanged.sort();
    Ok(report)
}

fn validate_workspace(workspace: ResourceWorkspace<'_>) -> Result<(), ResourceError> {
    if !workspace.backend_root.is_dir() {
        return Err(ResourceError::file(
            workspace.backend_root,
            "后端工作区不存在或不是目录",
            "传入包含 Cargo.toml 的后端仓库根目录",
        ));
    }
    if !workspace.backend_root.join("Cargo.toml").is_file() {
        return Err(ResourceError::file(
            workspace.backend_root,
            "后端工作区缺少 Cargo.toml",
            "传入后端 Cargo Workspace 根目录，而不是其父目录或子目录",
        ));
    }
    if let Some(frontend) = workspace.frontend_root
        && !frontend.is_dir()
    {
        return Err(ResourceError::file(
            frontend,
            "前端工作区不存在或不是目录",
            "传入前端仓库根目录",
        ));
    }
    Ok(())
}

fn load_manifest(path: &Path) -> Result<OwnershipManifest, ResourceError> {
    load_manifest_snapshot(path).map(|(manifest, _)| manifest)
}

fn verify_unselected_resource_sources(
    catalog: &GeneratedCatalog,
    selected: &str,
    manifest: &OwnershipManifest,
) -> Result<(), ResourceError> {
    let mut managed_hashes = BTreeMap::<&str, BTreeSet<&str>>::new();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.resource != selected && entry.resource != "__catalog__")
    {
        managed_hashes
            .entry(&entry.resource)
            .or_default()
            .insert(&entry.source_hash);
    }
    for (resource, hashes) in managed_hashes {
        if hashes.len() != 1 {
            return Err(ResourceError::new(
                "同一受管资源记录了不一致的清单哈希",
                "从版本控制恢复该资源全部生成文件与 ownership manifest 后重新生成",
            )
            .with_resource(resource)
            .with_file(MANIFEST_PATH));
        }
        let current = catalog.resources.get(resource).ok_or_else(|| {
            ResourceError::new(
                "未选中的受管资源缺少当前清单，不能安全刷新聚合入口",
                format!("恢复 catalog/resources/{resource}.toml；移除资源应使用显式删除流程"),
            )
            .with_resource(resource)
            .with_file(MANIFEST_PATH)
        })?;
        let owned_hash = hashes
            .first()
            .expect("非空受管资源哈希集合已由 manifest 条目建立");
        if current.source_hash != *owned_hash {
            return Err(ResourceError::new(
                "未选中的受管资源清单已有待生成变化，拒绝提前刷新中央聚合",
                format!("先运行 cargo resource {resource} --write，再重新生成所选资源 {selected}"),
            )
            .with_resource(resource)
            .with_file(&current.source_path));
        }
    }
    Ok(())
}

fn verify_no_resource_removal(
    catalog: &GeneratedCatalog,
    manifest: &OwnershipManifest,
) -> Result<(), ResourceError> {
    let missing = manifest
        .entries
        .iter()
        .filter(|entry| entry.resource != "__catalog__")
        .map(|entry| entry.resource.as_str())
        .filter(|resource| !catalog.resources.contains_key(*resource))
        .collect::<BTreeSet<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    let names = missing.into_iter().collect::<Vec<_>>().join(", ");
    Err(ResourceError::new(
        format!("当前 catalog 缺少已受管资源：{names}；拒绝把缺失清单推断为删除"),
        "当前版本尚不支持资源退役；恢复对应 TOML。后续显式 remove 流程必须保留不可变初始迁移并预览全部删除后才能执行",
    )
    .with_file(MANIFEST_PATH))
}

fn load_manifest_snapshot(
    path: &Path,
) -> Result<(OwnershipManifest, Option<Vec<u8>>), ResourceError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((OwnershipManifest::default(), None));
        }
        Err(error) => {
            return Err(ResourceError::file(
                path,
                format!("无法读取 ownership manifest：{error}"),
                "恢复该文件或删除全部受管生成目录后重新生成",
            ));
        }
    };
    let source = std::str::from_utf8(&bytes).map_err(|error| {
        ResourceError::file(
            path,
            format!("ownership manifest 不是 UTF-8：{error}"),
            "从版本控制恢复 manifest，不要手工编辑",
        )
    })?;
    let manifest = toml::from_str::<OwnershipManifest>(source).map_err(|error| {
        ResourceError::file(
            path,
            format!("ownership manifest 格式错误：{error}"),
            "从版本控制恢复 manifest，不要手工编辑",
        )
    })?;
    if manifest.format_version != 1 {
        return Err(ResourceError::file(
            path,
            format!(
                "不支持 ownership format_version {}",
                manifest.format_version
            ),
            "使用匹配项目版本的生成器重新生成",
        ));
    }
    let mut paths = BTreeSet::new();
    for entry in &manifest.entries {
        validate_managed_path(entry.root, &entry.path)?;
        if !paths.insert((entry.root, entry.path.as_str())) {
            return Err(ResourceError::file(
                path,
                format!("ownership 中路径 `{}` 重复", entry.path),
                "从版本控制恢复 manifest，不要手工合并条目",
            ));
        }
    }
    Ok((manifest, Some(bytes)))
}

fn verify_owned_files(
    manifest: &OwnershipManifest,
    workspace: ResourceWorkspace<'_>,
) -> Result<(), ResourceError> {
    for entry in &manifest.entries {
        let path = target_path(workspace, entry.root, &entry.path)?;
        let bytes = fs::read(&path).map_err(|error| {
            ResourceError::new(
                format!("受管文件丢失或无法读取：{error}"),
                "从版本控制恢复文件，或同时恢复匹配的 ownership manifest",
            )
            .with_resource(&entry.resource)
            .with_file(path.to_string_lossy())
        })?;
        if content_hash(&bytes) != entry.content_hash {
            return Err(ResourceError::new(
                "检测到对生成文件的人工修改，已停止且未写入任何文件",
                "把改动迁移到资源清单或强类型扩展，再从版本控制恢复生成文件",
            )
            .with_resource(&entry.resource)
            .with_file(path.to_string_lossy()));
        }
    }
    Ok(())
}

fn desired_entries(
    assets: &[GeneratedAsset],
    resources: &BTreeMap<String, super::ResourceIr>,
) -> Vec<OwnershipEntry> {
    let mut entries = assets
        .iter()
        .map(|asset| {
            let resource = resources.get(&asset.resource);
            OwnershipEntry {
                resource: asset.resource.clone(),
                root: asset.root,
                path: asset.path.clone(),
                source_hash: resource.map_or_else(
                    || extract_source_hash(&asset.content),
                    |resource| resource.source_hash.clone(),
                ),
                schema_hash: resource.map(|resource| resource.schema_hash.clone()),
                schema_revision: resource.and_then(|resource| resource.schema_revision.clone()),
                content_hash: content_hash(asset.content.as_bytes()),
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.root.cmp(&right.root).then(left.path.cmp(&right.path)));
    entries
}

fn manifests_equal(old: &OwnershipManifest, desired: &[OwnershipEntry]) -> bool {
    old.format_version == 1
        && old.generator_version == crate::GENERATOR_VERSION
        && old.entries == desired
}

fn target_path(
    workspace: ResourceWorkspace<'_>,
    root: AssetRoot,
    relative: &str,
) -> Result<PathBuf, ResourceError> {
    validate_managed_path(root, relative)?;
    let base = match root {
        AssetRoot::Backend => workspace.backend_root,
        AssetRoot::Frontend => workspace.frontend_root.ok_or_else(|| {
            ResourceError::new(
                "需要写入前端资产，但没有提供前端工作区",
                "设置 ResourceWorkspace.frontend_root",
            )
            .with_file(relative)
        })?,
    };
    ensure_no_symlink_escape(base, relative)?;
    Ok(base.join(relative))
}

fn validate_managed_path(root: AssetRoot, relative: &str) -> Result<(), ResourceError> {
    let path = Path::new(relative);
    if relative.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ResourceError::new(
            "生成路径不是安全的正斜杠相对路径",
            "移除盘符、反斜杠、空片段、`.` 与 `..`",
        )
        .with_file(relative));
    }
    let allowed = match root {
        AssetRoot::Backend => {
            relative == "catalog/access.generated.toml"
                || [
                    "crates/ryframe-application/src/generated/",
                    "crates/ryframe-db/src/generated/",
                    "crates/ryframe-api/src/generated/",
                    "crates/ryframe-tenant-db/src/generated/",
                ]
                .iter()
                .any(|prefix| relative.starts_with(prefix))
        }
        AssetRoot::Frontend => relative.starts_with("src/generated/resources/"),
    };
    if !allowed {
        return Err(ResourceError::new(
            "生成路径超出 ownership 白名单",
            "后端只写各 crate 的 src/generated 与 access.generated.toml；前端只写 src/generated/resources",
        )
        .with_file(relative));
    }
    Ok(())
}

fn ensure_no_symlink_escape(root: &Path, relative: &str) -> Result<(), ResourceError> {
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        ResourceError::file(
            root,
            format!("无法解析工作区根目录：{error}"),
            "确认路径存在且不是失效链接",
        )
    })?;
    let target = root.join(relative);
    let mut ancestor = target.parent().unwrap_or(root);
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| {
            ResourceError::file(
                &target,
                "生成路径没有有效父目录",
                "检查工作区根目录和生成路径",
            )
        })?;
    }
    let canonical_ancestor = fs::canonicalize(ancestor).map_err(|error| {
        ResourceError::file(
            ancestor,
            format!("无法解析生成目录：{error}"),
            "移除失效符号链接后重试",
        )
    })?;
    if !canonical_ancestor.starts_with(canonical_root) {
        return Err(ResourceError::file(
            &target,
            "生成路径通过链接逃逸出工作区",
            "移除生成目录中的外部符号链接",
        ));
    }
    Ok(())
}

fn display_path(root: AssetRoot, path: &str) -> String {
    format!("{}:{path}", root.label())
}

fn extract_source_hash(content: &str) -> String {
    if let Some((_, rest)) = content.split_once("source-sha256: ") {
        return rest
            .split(|character: char| character.is_whitespace() || character == '|')
            .next()
            .unwrap_or_default()
            .to_owned();
    }
    content_hash(content.as_bytes())
}

fn content_hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
