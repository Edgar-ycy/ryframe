use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{BusinessGenerateReport, BusinessPackage};
use crate::ResourceError;

const OWNERSHIP_PATH: &str = ".ryframe/generated.toml";

#[derive(Clone, Debug)]
pub(super) struct BusinessAsset {
    pub(super) resource: String,
    pub(super) path: String,
    pub(super) content: String,
}

impl BusinessAsset {
    pub(super) fn new(
        resource: impl Into<String>,
        path: impl Into<String>,
        content: String,
    ) -> Self {
        Self {
            resource: resource.into(),
            path: path.into(),
            content,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct OwnershipManifest {
    #[serde(default = "format_version")]
    format_version: u16,
    #[serde(default)]
    entries: Vec<OwnershipEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OwnershipEntry {
    resource: String,
    path: String,
    content_hash: String,
}

pub(super) fn install_assets(
    package: &BusinessPackage,
    selected: &BTreeSet<String>,
    mut assets: Vec<BusinessAsset>,
    write: bool,
) -> Result<BusinessGenerateReport, ResourceError> {
    assets.sort_by(|left, right| left.path.cmp(&right.path));
    ensure_unique_safe_paths(&assets)?;
    let manifest_path = package.root.join(OWNERSHIP_PATH);
    let old = load_manifest(&manifest_path)?;
    verify_owned_files(&package.root, &old)?;
    let old_by_path = old
        .entries
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let desired_paths = assets
        .iter()
        .map(|asset| asset.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut report = BusinessGenerateReport::default();
    for asset in &assets {
        let target = package.root.join(&asset.path);
        let hash = hash(asset.content.as_bytes());
        match old_by_path.get(asset.path.as_str()) {
            Some(entry) if entry.content_hash == hash => report.unchanged.push(asset.path.clone()),
            Some(_) if asset.path.starts_with("migrations/") => {
                return Err(error(
                    format!("迁移 {} 已生成，禁止覆盖；请新增手写迁移", asset.path),
                    &target,
                ));
            }
            Some(_) => report.updated.push(asset.path.clone()),
            None if target.exists() => {
                return Err(error(
                    format!("生成目标 {} 已存在但不属于生成器", asset.path),
                    &target,
                ));
            }
            None => report.created.push(asset.path.clone()),
        }
    }
    for entry in &old.entries {
        if !desired_paths.contains(entry.path.as_str())
            && (selected.contains(&entry.resource) || entry.resource == "__catalog__")
        {
            if entry.path.starts_with("migrations/") {
                return Err(error(
                    format!("迁移 {} 已发布，生成器不会删除或改名", entry.path),
                    &package.root.join(&entry.path),
                ));
            }
            report.removed.push(entry.path.clone());
        }
    }
    sort_report(&mut report);
    if !write {
        return Ok(report);
    }
    let desired_entries = merge_entries(&old, selected, &assets, &report.removed);
    let manifest = OwnershipManifest {
        format_version: format_version(),
        entries: desired_entries,
    };
    let manifest_source = toml::to_string_pretty(&manifest).map_err(|serialize| {
        error(
            format!("无法序列化生成 ownership：{serialize}"),
            &manifest_path,
        )
    })?;
    apply_changes(
        &package.root,
        &assets,
        &report.removed,
        manifest_source.as_bytes(),
    )?;
    Ok(report)
}

fn merge_entries(
    old: &OwnershipManifest,
    selected: &BTreeSet<String>,
    assets: &[BusinessAsset],
    removed: &[String],
) -> Vec<OwnershipEntry> {
    let removed = removed.iter().map(String::as_str).collect::<BTreeSet<_>>();
    let desired = assets
        .iter()
        .map(|asset| (asset.path.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    let mut entries = old
        .entries
        .iter()
        .filter(|entry| {
            !removed.contains(entry.path.as_str())
                && !desired.contains_key(entry.path.as_str())
                && !selected.contains(&entry.resource)
                && entry.resource != "__catalog__"
        })
        .cloned()
        .collect::<Vec<_>>();
    entries.extend(assets.iter().map(|asset| OwnershipEntry {
        resource: asset.resource.clone(),
        path: asset.path.clone(),
        content_hash: hash(asset.content.as_bytes()),
    }));
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    entries
}

fn apply_changes(
    root: &Path,
    assets: &[BusinessAsset],
    removed: &[String],
    manifest: &[u8],
) -> Result<(), ResourceError> {
    let stage = tempfile::Builder::new()
        .prefix(".ryframe-stage-")
        .tempdir_in(root)
        .map_err(|io| error(format!("无法创建同卷暂存目录：{io}"), root))?;
    for asset in assets {
        let staged = stage.path().join(&asset.path);
        if let Some(parent) = staged.parent() {
            fs::create_dir_all(parent)
                .map_err(|io| error(format!("无法创建暂存目录：{io}"), parent))?;
        }
        fs::write(&staged, asset.content.as_bytes())
            .map_err(|io| error(format!("无法写入暂存文件：{io}"), &staged))?;
    }
    let staged_manifest = stage.path().join(OWNERSHIP_PATH);
    if let Some(parent) = staged_manifest.parent() {
        fs::create_dir_all(parent)
            .map_err(|io| error(format!("无法创建 ownership 暂存目录：{io}"), parent))?;
    }
    fs::write(&staged_manifest, manifest)
        .map_err(|io| error(format!("无法暂存 ownership：{io}"), &staged_manifest))?;
    let backup = stage.path().join("backup");
    let mut installed = Vec::new();
    let result = (|| {
        for path in removed
            .iter()
            .chain(assets.iter().map(|asset| &asset.path))
            .map(String::as_str)
            .chain(std::iter::once(OWNERSHIP_PATH))
        {
            let target = root.join(path);
            if target.exists() {
                let saved = backup.join(path);
                if let Some(parent) = saved.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|io| error(format!("无法创建备份目录：{io}"), parent))?;
                }
                fs::rename(&target, &saved)
                    .map_err(|io| error(format!("无法备份受管文件：{io}"), &target))?;
            }
        }
        for asset in assets {
            let target = root.join(&asset.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .map_err(|io| error(format!("无法创建目标目录：{io}"), parent))?;
            }
            fs::rename(stage.path().join(&asset.path), &target)
                .map_err(|io| error(format!("无法安装生成文件：{io}"), &target))?;
            installed.push(asset.path.as_str());
        }
        let manifest_target = root.join(OWNERSHIP_PATH);
        if let Some(parent) = manifest_target.parent() {
            fs::create_dir_all(parent)
                .map_err(|io| error(format!("无法创建 ownership 目录：{io}"), parent))?;
        }
        fs::rename(staged_manifest, &manifest_target)
            .map_err(|io| error(format!("无法安装 ownership：{io}"), &manifest_target))?;
        installed.push(OWNERSHIP_PATH);
        Ok(())
    })();
    if let Err(failure) = result {
        for path in installed.into_iter().rev() {
            let target = root.join(path);
            let _ = fs::remove_file(target);
        }
        for path in removed
            .iter()
            .chain(assets.iter().map(|asset| &asset.path))
            .map(String::as_str)
            .chain(std::iter::once(OWNERSHIP_PATH))
        {
            let saved = backup.join(path);
            if saved.exists() {
                let target = root.join(path);
                if let Some(parent) = target.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                let _ = fs::rename(saved, target);
            }
        }
        return Err(failure);
    }
    Ok(())
}

fn load_manifest(path: &Path) -> Result<OwnershipManifest, ResourceError> {
    match fs::read_to_string(path) {
        Ok(source) => {
            let manifest: OwnershipManifest = toml::from_str(&source)
                .map_err(|parse| error(format!("ownership 格式错误：{parse}"), path))?;
            if manifest.format_version != format_version() {
                return Err(error("不支持的 ownership 版本", path));
            }
            Ok(manifest)
        }
        Err(io) if io.kind() == std::io::ErrorKind::NotFound => Ok(OwnershipManifest::default()),
        Err(io) => Err(error(format!("无法读取 ownership：{io}"), path)),
    }
}

fn verify_owned_files(root: &Path, manifest: &OwnershipManifest) -> Result<(), ResourceError> {
    for entry in &manifest.entries {
        validate_path(&entry.path)?;
        let path = root.join(&entry.path);
        let bytes =
            fs::read(&path).map_err(|io| error(format!("受管生成文件丢失：{io}"), &path))?;
        if hash(&bytes) != entry.content_hash {
            return Err(error("受管生成文件被人工修改", &path));
        }
    }
    Ok(())
}

fn ensure_unique_safe_paths(assets: &[BusinessAsset]) -> Result<(), ResourceError> {
    let mut paths = BTreeSet::new();
    for asset in assets {
        validate_path(&asset.path)?;
        if !paths.insert(asset.path.as_str()) {
            return Err(error(
                format!("生成路径重复：{}", asset.path),
                Path::new(&asset.path),
            ));
        }
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), ResourceError> {
    let parsed = Path::new(path);
    let safe = !path.contains('\\')
        && !parsed.is_absolute()
        && parsed
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && (path.starts_with("src/generated/") || path.starts_with("migrations/"));
    if safe {
        Ok(())
    } else {
        Err(error("生成路径超出业务 crate 白名单", parsed))
    }
}

fn hash(content: &[u8]) -> String {
    hex::encode(Sha256::digest(content))
}

const fn format_version() -> u16 {
    1
}

fn sort_report(report: &mut BusinessGenerateReport) {
    report.created.sort();
    report.updated.sort();
    report.removed.sort();
    report.unchanged.sort();
}

fn error(message: impl Into<String>, path: &Path) -> ResourceError {
    ResourceError::new(
        message,
        "从版本控制恢复受管文件，或修改 ResourceModel 后重新生成",
    )
    .with_file(path.to_string_lossy())
}
