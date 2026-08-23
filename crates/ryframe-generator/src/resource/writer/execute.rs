use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::PathBuf,
};

use super::{
    AssetRoot, ExpectedFile, GeneratedAsset, GeneratedCatalog, InstalledFile, MANIFEST_PATH,
    OwnershipManifest, ResourceError, ResourceWorkspace, SafeWriteReport, content_hash,
    desired_entries, display_path, install_staged, load_manifest, load_manifest_snapshot,
    manifests_equal, move_to_backup, persist_recovery_directories, rollback, schema,
    selected_assets, target_path, validate_workspace, verify_expected_file,
    verify_no_resource_removal, verify_owned_files, verify_unselected_resource_sources,
    write_staged,
};

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
