use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{BusinessGenerateReport, BusinessPackage};
use crate::ResourceError;

pub(crate) mod staged;
use crate::resource::write_transaction as transaction;
use staged::apply_changes;
use transaction::ExpectedFile;

const OWNERSHIP_PATH: &str = ".ryframe/generated.toml";

#[derive(Clone, Debug)]
pub(crate) struct BusinessAsset {
    pub(crate) resource: String,
    pub(crate) path: String,
    pub(crate) content: String,
}

impl BusinessAsset {
    pub(crate) fn new(
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
    let (old, manifest_expected) = load_manifest(&manifest_path)?;
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
        if asset.resource != "__catalog__"
            && !selected.contains(&asset.resource)
            && old_by_path
                .get(asset.path.as_str())
                .is_none_or(|entry| entry.content_hash != hash)
        {
            return Err(error(
                format!(
                    "未选中的资源 {} 仍有待生成差异：{}；本批未写入",
                    asset.resource, asset.path
                ),
                &target,
            ));
        }
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
            && !selected.contains(&entry.resource)
            && entry.resource != "__catalog__"
        {
            return Err(error(
                format!(
                    "未选中的资源 {} 存在待删除资产 {}；请完整预览后再生成",
                    entry.resource, entry.path
                ),
                &package.root,
            ));
        }
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
    if !write
        || (report.created.is_empty() && report.updated.is_empty() && report.removed.is_empty())
    {
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
    let expected = expected_files(&old, &assets, manifest_expected);
    let changed = assets
        .iter()
        .filter(|asset| {
            report.created.contains(&asset.path) || report.updated.contains(&asset.path)
        })
        .cloned()
        .collect::<Vec<_>>();
    apply_changes(
        &package.root,
        &changed,
        &report.removed,
        &manifest_source,
        &expected,
    )?;
    Ok(report)
}

pub(super) fn verify_package(package: &BusinessPackage) -> Result<(), ResourceError> {
    let (manifest, _) = load_manifest(&package.root.join(OWNERSHIP_PATH))?;
    verify_owned_files(&package.root, &manifest)
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

fn expected_files(
    old: &OwnershipManifest,
    assets: &[BusinessAsset],
    manifest: ExpectedFile,
) -> BTreeMap<String, ExpectedFile> {
    let mut expected = old
        .entries
        .iter()
        .map(|entry| {
            (
                entry.path.clone(),
                ExpectedFile::ContentHash(entry.content_hash.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    for asset in assets {
        expected
            .entry(asset.path.clone())
            .or_insert(ExpectedFile::Absent);
    }
    expected.insert(OWNERSHIP_PATH.into(), manifest);
    expected
}

fn load_manifest(path: &Path) -> Result<(OwnershipManifest, ExpectedFile), ResourceError> {
    match fs::read_to_string(path) {
        Ok(source) => {
            let manifest: OwnershipManifest = toml::from_str(&source)
                .map_err(|parse| error(format!("ownership 格式错误：{parse}"), path))?;
            if manifest.format_version != format_version() {
                return Err(error("不支持的 ownership 版本", path));
            }
            Ok((manifest, ExpectedFile::ExactBytes(source.into_bytes())))
        }
        Err(io) if io.kind() == std::io::ErrorKind::NotFound => {
            Ok((OwnershipManifest::default(), ExpectedFile::Absent))
        }
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
