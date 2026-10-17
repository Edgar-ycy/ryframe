use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::{
    policy::{Edge, Profile, cyclic_components, format_edges},
    support::{command_stdout, display_relative},
};

#[derive(Debug, Deserialize)]
pub(super) struct Metadata {
    packages: Vec<Package>,
    workspace_members: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Package {
    pub(crate) name: String,
    pub(crate) id: String,
    pub(crate) manifest_path: PathBuf,
    #[serde(default)]
    pub(crate) dependencies: Vec<Dependency>,
    #[serde(default)]
    pub(crate) targets: Vec<Target>,
    pub(crate) metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Dependency {
    pub(crate) path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Target {
    pub(crate) name: Option<String>,
    pub(crate) kind: Vec<String>,
    pub(crate) src_path: PathBuf,
    pub(crate) test: Option<bool>,
}

pub(crate) type Packages = BTreeMap<String, Package>;

pub(crate) fn load_packages(root: &Path, errors: &mut Vec<String>) -> Packages {
    let output = match command_stdout(
        root,
        "cargo".as_ref(),
        &[
            "metadata",
            "--locked",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ],
        None,
    ) {
        Ok(output) => output,
        Err(error) => {
            errors.push(format!("cargo metadata 执行失败: {error}"));
            return Packages::new();
        }
    };
    let metadata = match serde_json::from_str::<Metadata>(&output) {
        Ok(metadata) => metadata,
        Err(error) => {
            errors.push(format!("cargo metadata 输出不是有效 JSON: {error}"));
            return Packages::new();
        }
    };
    let members = metadata
        .workspace_members
        .into_iter()
        .collect::<BTreeSet<_>>();
    metadata
        .packages
        .into_iter()
        .filter(|package| members.contains(&package.id))
        .map(|package| (package.name.clone(), package))
        .collect()
}

pub(crate) fn validate_unsafe_lint_policy(
    root: &Path,
    packages: &Packages,
    errors: &mut Vec<String>,
) -> usize {
    let manifest = match fs::read_to_string(root.join("Cargo.toml"))
        .ok()
        .and_then(|source| toml::from_str::<toml::Value>(&source).ok())
    {
        Some(manifest) => manifest,
        None => {
            errors.push("无法读取 Workspace unsafe lint".into());
            return 0;
        }
    };
    let forbidden = manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("lints"))
        .and_then(toml::Value::as_table)
        .and_then(|lints| lints.get("rust"))
        .and_then(toml::Value::as_table)
        .and_then(|rust| rust.get("unsafe_code"))
        .and_then(toml::Value::as_str)
        == Some("forbid");
    if !forbidden {
        errors.push("Workspace 必须设置 [workspace.lints.rust] unsafe_code = \"forbid\"".into());
    }
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut checked = 0;
    for (name, package) in packages {
        let manifest_path = canonical_manifest(&package.manifest_path);
        if !manifest_path.starts_with(&canonical_root) {
            errors.push(format!("工作区 crate 位于仓库之外：{name}"));
            continue;
        }
        let Some(manifest) = fs::read_to_string(&manifest_path)
            .ok()
            .and_then(|source| toml::from_str::<toml::Value>(&source).ok())
        else {
            errors.push(format!("无法读取 crate lint 配置：{name}"));
            continue;
        };
        checked += 1;
        if manifest
            .get("lints")
            .and_then(toml::Value::as_table)
            .and_then(|lints| lints.get("workspace"))
            .and_then(toml::Value::as_bool)
            != Some(true)
        {
            errors.push(format!("工作区 crate 必须继承 Workspace lint：{name}"));
        }
    }
    checked
}

pub(crate) fn validate_active_workspace(
    root: &Path,
    active_name: &str,
    profile: &Profile,
    packages: &Packages,
    errors: &mut Vec<String>,
) -> BTreeSet<Edge> {
    let actual_packages = packages.keys().cloned().collect::<BTreeSet<_>>();
    let business = business_packages(root, packages, errors);
    let core = actual_packages
        .difference(&business)
        .cloned()
        .collect::<BTreeSet<_>>();
    if core.len() != profile.expected_count {
        errors.push(format!(
            "工作区核心包数漂移：profile={active_name} 期望 {}，实际 {}",
            profile.expected_count,
            core.len()
        ));
    }
    let missing = profile
        .packages
        .difference(&core)
        .cloned()
        .collect::<Vec<_>>();
    let unexpected = core
        .difference(&profile.packages)
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        errors.push(format!("工作区缺少 profile 包: {}", missing.join(", ")));
    }
    if !unexpected.is_empty() {
        errors.push(format!("工作区出现未批准包: {}", unexpected.join(", ")));
    }
    let actual_edges = workspace_edges(packages);
    validate_edges(&actual_edges, &core, &business, profile, errors);
    for component in cyclic_components(&actual_packages, &actual_edges) {
        errors.push(format!(
            "工作区内部依赖存在 SCC: {}",
            component.join(" -> ")
        ));
    }
    actual_edges
}

fn validate_edges(
    actual: &BTreeSet<Edge>,
    core: &BTreeSet<String>,
    business: &BTreeSet<String>,
    profile: &Profile,
    errors: &mut Vec<String>,
) {
    let core_edges = actual
        .iter()
        .filter(|(source, target)| core.contains(source) && core.contains(target))
        .cloned()
        .collect::<BTreeSet<_>>();
    let forbidden = core_edges
        .difference(&profile.allowed_edges)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !forbidden.is_empty() {
        errors.push(format!(
            "工作区出现未批准内部依赖边: {}",
            format_edges(&forbidden)
        ));
    }
    let stale = profile
        .allowed_edges
        .difference(&core_edges)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !stale.is_empty() {
        errors.push(format!(
            "架构事实源包含已不存在的内部依赖边: {}",
            format_edges(&stale)
        ));
    }
    let product_tool = core_edges
        .iter()
        .filter(|(source, target)| {
            profile.products.contains(source) && profile.tools.contains(target)
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let unapproved = product_tool
        .difference(&profile.temporary_edges)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !unapproved.is_empty() {
        errors.push(format!(
            "产品 crate 不得依赖工具 crate: {}",
            format_edges(&unapproved)
        ));
    }
    let stale_exceptions = profile
        .temporary_edges
        .difference(&product_tool)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !stale_exceptions.is_empty() {
        errors.push(format!(
            "产品 -> 工具临时豁免已失效，应删除: {}",
            format_edges(&stale_exceptions)
        ));
    }
    let invalid_business = actual
        .iter()
        .filter(|(source, target)| {
            let touches_business = business.contains(source) || business.contains(target);
            let allowed_business = (source == "ryframe" && business.contains(target))
                || (business.contains(source) && target == "ryframe-sdk")
                || (business.contains(source) && business.contains(target));
            touches_business && !allowed_business
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    if !invalid_business.is_empty() {
        errors.push(format!(
            "业务 crate 只能由 ryframe 组合根接入，并且只能依赖 ryframe-sdk 或其他业务 crate: {}",
            format_edges(&invalid_business)
        ));
    }
}

fn workspace_edges(packages: &Packages) -> BTreeSet<Edge> {
    let roots = packages
        .iter()
        .map(|(name, package)| {
            (
                canonical_manifest(&package.manifest_path)
                    .parent()
                    .unwrap_or(Path::new(""))
                    .to_path_buf(),
                name.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    packages
        .iter()
        .flat_map(|(name, package)| {
            package.dependencies.iter().filter_map(|dependency| {
                let path = dependency.path.as_ref()?.canonicalize().ok()?;
                let target = roots.get(&path)?;
                Some((name.clone(), target.clone()))
            })
        })
        .collect()
}

fn business_packages(
    root: &Path,
    packages: &Packages,
    errors: &mut Vec<String>,
) -> BTreeSet<String> {
    let crates = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .join("crates");
    let mut result = BTreeSet::new();
    for (name, package) in packages {
        let Some(ryframe) = package
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("ryframe"))
        else {
            continue;
        };
        if ryframe.get("kind").and_then(serde_json::Value::as_str) != Some("business") {
            continue;
        }
        if !ryframe
            .get("module")
            .and_then(serde_json::Value::as_str)
            .is_some_and(valid_module_name)
        {
            errors.push(format!("业务 crate {name} 的 metadata.ryframe.module 无效"));
        }
        let manifest = canonical_manifest(&package.manifest_path);
        if !manifest.starts_with(&crates) {
            errors.push(format!("业务 crate {name} 必须位于 crates/ 下"));
        }
        result.insert(name.clone());
    }
    result
}

fn valid_module_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase())
        && chars.all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        })
}

fn canonical_manifest(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn report_outside_repository(name: &str, package_root: &Path, errors: &mut Vec<String>) {
    errors.push(format!(
        "工作区 crate 位于仓库之外: {name}（{}）",
        display_relative(package_root)
    ));
}
