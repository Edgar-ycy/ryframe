use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use super::{
    GeneratedAsset, GeneratedCatalog, OwnershipManifest, ResourceError, ResourceWorkspace,
    target_path,
};
use crate::StorageKind;

pub(super) fn preserve_initial_migrations(
    mut assets: Vec<GeneratedAsset>,
    manifest: &OwnershipManifest,
    workspace: ResourceWorkspace<'_>,
    require_all: bool,
) -> Result<Vec<GeneratedAsset>, ResourceError> {
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| is_initial_migration(&entry.path))
    {
        let desired_index = assets
            .iter_mut()
            .position(|asset| asset.root == entry.root && asset.path == entry.path);
        let Some(desired_index) = desired_index else {
            if !require_all && !assets.iter().any(|asset| asset.resource == entry.resource) {
                continue;
            }
            return Err(ResourceError::new(
                "已落盘的资源初始迁移不可删除或改名",
                "恢复 database.bootstrap_migration=true 与原生成路径；后续变更使用追加 roll-forward 迁移",
            )
            .with_resource(&entry.resource)
            .with_file(&entry.path));
        };
        let path = target_path(workspace, entry.root, &entry.path)?;
        assets[desired_index].content = fs::read_to_string(&path).map_err(|error| {
            ResourceError::file(
                &path,
                format!("无法保留不可变资源初始迁移：{error}"),
                "从版本控制恢复初始迁移与 ownership manifest 后重试",
            )
            .with_resource(&entry.resource)
        })?;
    }
    Ok(assets)
}

pub(super) fn validate_evolution(
    catalog: &GeneratedCatalog,
    manifest: &OwnershipManifest,
    workspace: ResourceWorkspace<'_>,
) -> Result<(), ResourceError> {
    let managed = managed_schemas(manifest)?;
    for resource in catalog.resources.values() {
        let Some((previous_hash, previous_revision)) = managed.get(resource.name.as_str()) else {
            continue;
        };
        if previous_hash == &resource.schema_hash {
            if previous_revision.as_deref() != resource.schema_revision.as_deref() {
                return Err(ResourceError::new(
                    "schema_revision 发生变化，但持久化 schema 没有变化",
                    "删除无效的 schema_revision 改动；只有真实 schema 变化才绑定新的追加迁移",
                )
                .with_resource(&resource.name)
                .with_file(&resource.source_path));
            }
            continue;
        }
        let revision = resource.schema_revision.as_deref().ok_or_else(|| {
            ResourceError::new(
                "持久化 schema 已变化，但没有声明新的 schema_revision",
                schema_change_suggestion(resource.storage),
            )
            .with_resource(&resource.name)
            .with_file(&resource.source_path)
        })?;
        if previous_revision.as_deref() == Some(revision) {
            return Err(ResourceError::new(
                format!("schema_revision `{revision}` 已用于上一版 schema"),
                schema_change_suggestion(resource.storage),
            )
            .with_resource(&resource.name)
            .with_file(&resource.source_path));
        }
        let migration_files =
            find_revision_files(workspace.backend_root, resource.storage, revision).ok_or_else(
                || {
                    ResourceError::new(
                        format!("找不到 schema_revision `{revision}` 对应的追加迁移"),
                        schema_change_suggestion(resource.storage),
                    )
                    .with_resource(&resource.name)
                    .with_file(&resource.source_path)
                },
            )?;
        verify_revision_state(workspace.backend_root, &migration_files, &resource.name)?;
    }
    Ok(())
}

fn managed_schemas(
    manifest: &OwnershipManifest,
) -> Result<BTreeMap<&str, (String, Option<String>)>, ResourceError> {
    let mut managed = BTreeMap::new();
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.resource != "__catalog__")
    {
        let schema_hash = entry.schema_hash.as_ref().ok_or_else(|| {
            ResourceError::new(
                "ownership 缺少持久化 schema_hash，不能安全判断迁移边界",
                "从版本控制恢复一致的生成资产与 ownership manifest，再用当前生成器重新建立资源",
            )
            .with_resource(&entry.resource)
            .with_file(&entry.path)
        })?;
        let value = (schema_hash.clone(), entry.schema_revision.clone());
        if let Some(previous) = managed.insert(entry.resource.as_str(), value.clone())
            && previous != value
        {
            return Err(ResourceError::new(
                "同一资源的 ownership schema 状态不一致",
                "从版本控制恢复该资源全部生成文件与 ownership manifest 后重试",
            )
            .with_resource(&entry.resource)
            .with_file(&entry.path));
        }
    }
    Ok(managed)
}

fn find_revision_files(root: &Path, storage: StorageKind, revision: &str) -> Option<Vec<PathBuf>> {
    let directory = match storage {
        StorageKind::ControlRow => root.join("crates/ryframe-db/src/migration"),
        StorageKind::TenantData => root.join("crates/ryframe-tenant-db/src/migration"),
    };
    let file = directory.join(format!("{revision}.rs"));
    if file.is_file() {
        return Some(vec![file]);
    }
    let module = directory.join(revision).join("mod.rs");
    if module.is_file() {
        let mut files = fs::read_dir(module.parent().expect("迁移模块必须有父目录"))
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .collect::<Vec<_>>();
        files.sort();
        return Some(files);
    }
    None
}

fn verify_revision_state(
    root: &Path,
    files: &[PathBuf],
    resource: &str,
) -> Result<(), ResourceError> {
    let relative = files
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .expect("迁移文件必须位于后端根目录")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    let locked = locked_paths(root)?;
    if relative.iter().all(|path| locked.contains(path)) {
        return Ok(());
    }
    if !root.join(".git").exists() {
        return Ok(());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-tree", "-r", "--name-only", "HEAD", "--"])
        .args(&relative)
        .output()
        .map_err(|error| {
            ResourceError::new(
                format!("无法检查 schema_revision 的 Git 状态：{error}"),
                "确认 Git 可用后重试；生成器不会猜测迁移是否已冻结",
            )
            .with_resource(resource)
        })?;
    if !output.status.success() {
        return Err(ResourceError::new(
            "无法从 Git HEAD 检查 schema_revision，已拒绝生成",
            "修复仓库状态后重试；或先由 migration checker 冻结追加迁移",
        )
        .with_resource(resource));
    }
    if !output.stdout.is_empty() {
        return Err(ResourceError::new(
            "schema_revision 已进入 HEAD，但尚未全部写入 migrations.lock.toml",
            "不要补录已提交历史；从新的工作树追加 roll-forward 迁移，或恢复提交前正确冻结的 lock",
        )
        .with_resource(resource));
    }
    Ok(())
}

fn locked_paths(root: &Path) -> Result<BTreeSet<String>, ResourceError> {
    let path = root.join("catalog/migrations.lock.toml");
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => {
            return Err(ResourceError::file(
                &path,
                format!("无法读取迁移冻结清单：{error}"),
                "恢复 migrations.lock.toml 后重试",
            ));
        }
    };
    let value = toml::from_str::<toml::Value>(&source).map_err(|error| {
        ResourceError::file(
            &path,
            format!("迁移冻结清单格式错误：{error}"),
            "先通过 cargo xtask data migrate verify 修复冻结清单",
        )
    })?;
    Ok(value
        .get("files")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("path").and_then(toml::Value::as_str))
        .map(str::to_owned)
        .collect())
}

fn schema_change_suggestion(storage: StorageKind) -> &'static str {
    match storage {
        StorageKind::ControlRow => {
            "先运行 `cargo xtask data migrate new control <name>`，再把生成的完整迁移名写入 database.schema_revision"
        }
        StorageKind::TenantData => {
            "先运行 `cargo xtask data migrate new tenant-data <name>`，再把生成的完整迁移名写入 database.schema_revision"
        }
    }
}

fn is_initial_migration(path: &str) -> bool {
    (path.starts_with("crates/ryframe-db/src/generated/")
        || path.starts_with("crates/ryframe-tenant-db/src/generated/"))
        && path.ends_with("/migration.rs")
}
