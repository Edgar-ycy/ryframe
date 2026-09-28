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

mod changes;
mod staged;

use changes::ChangeSet;
use staged::StagedWrite;

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
    let mut changes = ChangeSet::prepare(workspace, &old_manifest, &desired_assets)?;
    if changes.changed.is_empty()
        && changes.obsolete.is_empty()
        && manifests_equal(&old_manifest, &desired_entries)
    {
        changes.report.unchanged.sort();
        return Ok(changes.report);
    }
    let staged = StagedWrite::prepare(workspace, &changes, desired_entries)?;
    let expected_manifest =
        old_manifest_bytes.map_or(ExpectedFile::Absent, ExpectedFile::ExactBytes);
    staged.commit(workspace, &old_manifest, expected_manifest, &mut changes)?;
    for entry in changes.obsolete {
        changes
            .report
            .removed
            .push(display_path(entry.root, &entry.path));
    }
    let mut report = changes.report;
    report.written.sort();
    report.created.sort();
    report.updated.sort();
    report.removed.sort();
    report.unchanged.sort();
    Ok(report)
}
