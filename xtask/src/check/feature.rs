use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use crate::{Result, workspace::root_dir};

use super::{
    execution::{BACKEND_VERIFY_TARGET_DIR, run_owned},
    selection::load_workspace_metadata,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FeatureMatrixEntry {
    pub(super) package: String,
    pub(super) minimal: Vec<String>,
    pub(super) maximal: Vec<String>,
    pub(super) test_targets: Vec<String>,
}

pub(crate) fn feature_matrix() -> Result<()> {
    let jobs = std::thread::available_parallelism().map_or(2, usize::from);
    feature_matrix_with_jobs(jobs)
}

pub(crate) fn feature_matrix_with_jobs(jobs: usize) -> Result<()> {
    let root = root_dir();
    let metadata = load_workspace_metadata(&root)?;
    let registry = load_feature_registry(&root)?;
    validate_feature_registry(&metadata, &registry)?;

    println!("检查 Workspace 的最小 feature 组合。");
    run_owned(&root, "cargo", &minimal_workspace_check_args(jobs))
}

pub(crate) fn minimal_workspace_check_args(jobs: usize) -> Vec<String> {
    [
        "check",
        "--locked",
        "--target-dir",
        BACKEND_VERIFY_TARGET_DIR,
        "--workspace",
        "--no-default-features",
        "--all-targets",
        "--jobs",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([jobs.to_string()])
    .collect()
}

pub(super) fn check_feature_registry(root: &Path) -> Result<()> {
    let metadata = load_workspace_metadata(root)?;
    let registry = load_feature_registry(root)?;
    validate_feature_registry(&metadata, &registry)?;
    println!("Cargo feature 注册表检查通过。");
    Ok(())
}

pub(super) fn load_feature_registry(root: &Path) -> Result<Vec<FeatureMatrixEntry>> {
    let path = root.join("config/feature-matrix.json");
    let source = fs::read(&path)
        .map_err(|error| format!("无法读取 feature 注册表 {}：{error}", path.display()))?;
    let document: serde_json::Value = serde_json::from_slice(&source)
        .map_err(|error| format!("feature 注册表不是有效 JSON：{error}"))?;
    if document.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err("feature 注册表 version 必须为 1".into());
    }
    let packages = document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("feature 注册表缺少 packages 数组")?;

    packages
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let package = entry
                .get("package")
                .and_then(serde_json::Value::as_str)
                .filter(|package| !package.trim().is_empty())
                .ok_or_else(|| format!("feature 注册表 packages[{index}] 缺少 package"))?;
            Ok(FeatureMatrixEntry {
                package: package.to_owned(),
                minimal: feature_names(entry.get("minimal"), index, "minimal")?,
                maximal: feature_names(entry.get("maximal"), index, "maximal")?,
                test_targets: feature_names(entry.get("test_targets"), index, "test_targets")?,
            })
        })
        .collect()
}

fn feature_names(
    value: Option<&serde_json::Value>,
    index: usize,
    field: &str,
) -> Result<Vec<String>> {
    value
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("feature 注册表 packages[{index}].{field} 必须是数组"))?
        .iter()
        .enumerate()
        .map(|(feature_index, value)| {
            value
                .as_str()
                .filter(|feature| !feature.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "feature 注册表 packages[{index}].{field}[{feature_index}] 必须是非空字符串"
                    )
                    .into()
                })
        })
        .collect()
}

fn workspace_features(metadata: &serde_json::Value) -> Result<BTreeMap<String, BTreeSet<String>>> {
    let members = metadata
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 workspace_members")?
        .iter()
        .map(|member| {
            member
                .as_str()
                .map(str::to_owned)
                .ok_or("Cargo workspace_members 中存在非字符串成员")
        })
        .collect::<std::result::Result<BTreeSet<_>, _>>()?;
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 packages")?;
    let mut result = BTreeMap::new();

    for package in packages {
        let id = package
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 id")?;
        if !members.contains(id) {
            continue;
        }
        let name = package
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 name")?;
        let features = package
            .get("features")
            .and_then(serde_json::Value::as_object)
            .ok_or("Cargo package 缺少 features")?
            .keys()
            .cloned()
            .collect();
        if result.insert(name.to_owned(), features).is_some() {
            return Err(format!("Cargo 工作区存在重复包名：{name}").into());
        }
    }

    if result.len() != members.len() {
        return Err("Cargo 元数据未包含全部工作区成员".into());
    }
    Ok(result)
}

pub(super) fn validate_feature_registry(
    metadata: &serde_json::Value,
    registry: &[FeatureMatrixEntry],
) -> Result<()> {
    let packages = workspace_features(metadata)?;
    let feature_packages = packages
        .iter()
        .filter(|(_, features)| !features.is_empty())
        .map(|(name, _)| name.as_str())
        .collect::<BTreeSet<_>>();
    let mut registered_packages = BTreeSet::new();

    for entry in registry {
        if !registered_packages.insert(entry.package.as_str()) {
            return Err(format!("Cargo feature 注册表重复登记包：{}", entry.package).into());
        }
        let available = packages
            .get(entry.package.as_str())
            .ok_or_else(|| format!("Cargo feature 注册表包含未知包：{}", entry.package))?;
        if available.is_empty() {
            return Err(format!("包 {} 没有 feature，不应登记矩阵", entry.package).into());
        }

        let minimal =
            validate_feature_combination(&entry.package, "最小", &entry.minimal, available)?;
        let maximal =
            validate_feature_combination(&entry.package, "最大", &entry.maximal, available)?;
        if !minimal.is_subset(&maximal) {
            return Err(
                format!("包 {} 的最小 feature 组合不是最大组合的子集", entry.package).into(),
            );
        }
        if &maximal != available {
            let missing = available.difference(&maximal).cloned().collect::<Vec<_>>();
            return Err(format!(
                "包 {} 的最大 feature 组合未覆盖：{}",
                entry.package,
                missing.join(", ")
            )
            .into());
        }
        let test_targets = entry.test_targets.iter().collect::<BTreeSet<_>>();
        if test_targets.len() != entry.test_targets.len() {
            return Err(format!("包 {} 的 feature 测试目标包含重复项", entry.package).into());
        }
    }

    if registered_packages != feature_packages {
        let missing = feature_packages
            .difference(&registered_packages)
            .copied()
            .collect::<Vec<_>>();
        let extra = registered_packages
            .difference(&feature_packages)
            .copied()
            .collect::<Vec<_>>();
        return Err(format!(
            "Cargo feature 注册表与工作区不一致；未登记：[{}]，多余：[{}]",
            missing.join(", "),
            extra.join(", ")
        )
        .into());
    }
    Ok(())
}

pub(crate) fn validate_feature_combination(
    package: &str,
    label: &str,
    combination: &[String],
    available: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let selected = combination.iter().cloned().collect::<BTreeSet<_>>();
    if selected.len() != combination.len() {
        return Err(format!("包 {package} 的{label} feature 组合包含重复项").into());
    }
    let unknown = selected.difference(available).cloned().collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(format!(
            "包 {package} 的{label} feature 组合包含未知项：{}",
            unknown.join(", ")
        )
        .into());
    }
    Ok(selected)
}

pub(super) fn run_feature_operations(
    root: &Path,
    package: &str,
    label: &str,
    features: &[String],
    operations: &[&str],
    jobs: usize,
) -> Result<()> {
    println!("检查 {package} 的{label} feature 组合。");
    for operation in operations {
        let args = feature_operation_args(operation, package, features, jobs);
        run_owned(root, "cargo", &args)?;
    }
    Ok(())
}

pub(super) fn run_feature_tests(
    root: &Path,
    entry: &FeatureMatrixEntry,
    jobs: usize,
) -> Result<()> {
    for target in &entry.test_targets {
        println!(
            "检查 {} 的最大 feature 测试目标 {}。",
            entry.package, target
        );
        run_owned(
            root,
            "cargo",
            &feature_test_args(&entry.package, &entry.maximal, target, jobs),
        )?;
    }
    Ok(())
}

pub(crate) fn feature_test_args(
    package: &str,
    features: &[String],
    target: &str,
    jobs: usize,
) -> Vec<String> {
    let mut args = vec![
        "test".to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        BACKEND_VERIFY_TARGET_DIR.to_owned(),
        "-p".to_owned(),
        package.to_owned(),
        "--no-default-features".to_owned(),
        "--jobs".to_owned(),
        jobs.to_string(),
        "--test".to_owned(),
        target.to_owned(),
    ];
    if !features.is_empty() {
        args.extend(["--features".to_owned(), features.join(",")]);
    }
    args
}

pub(crate) fn feature_operation_args(
    operation: &str,
    package: &str,
    features: &[String],
    jobs: usize,
) -> Vec<String> {
    let mut args = vec![
        operation.to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        BACKEND_VERIFY_TARGET_DIR.to_owned(),
        "-p".to_owned(),
        package.to_owned(),
        "--no-default-features".to_owned(),
        "--jobs".to_owned(),
        jobs.to_string(),
    ];
    args.push("--all-targets".to_owned());
    if !features.is_empty() {
        args.extend(["--features".to_owned(), features.join(",")]);
    }
    if operation == "clippy" {
        args.extend([
            "--".to_owned(),
            "-D".to_owned(),
            "warnings".to_owned(),
            "-D".to_owned(),
            "clippy::redundant_clone".to_owned(),
        ]);
    }
    args
}
