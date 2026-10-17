use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    process::Stdio,
};

use regex::Regex;

use super::policy::{FeatureProfile, Policy};
use crate::process::child_command;

pub(crate) fn verify(root: &Path, policy: &Policy) -> Vec<String> {
    let mut errors = Vec::new();
    match cargo_tree(
        root,
        &[
            "tree",
            "--locked",
            "--workspace",
            "--all-features",
            "--target",
            "all",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
    ) {
        Ok(output) => errors.extend(resolved_graph_violations(&output, policy)),
        Err(error) => errors.push(format!("cargo tree 执行失败：{error}")),
    }
    for profile in &policy.runtime_feature_tree_gate.profiles {
        let arguments = feature_tree_args(profile);
        let borrowed = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        match cargo_tree(root, &borrowed) {
            Ok(output) => errors.extend(feature_tree_violations(&output, profile)),
            Err(error) => errors.push(format!(
                "{} 定向 cargo tree 执行失败：{error}",
                profile.name
            )),
        }
    }
    errors
}

pub(crate) fn resolved_graph_violations(output: &str, policy: &Policy) -> Vec<String> {
    let resolved = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<BTreeSet<_>>();
    policy
        .dependency_graph_exceptions
        .iter()
        .filter_map(|exception| {
            let identity = format!("{} v{}", exception.package, exception.version);
            resolved
                .contains(identity.as_str())
                .then(|| format!("条件依赖例外已进入实际构建图，必须升级：{identity}"))
        })
        .collect()
}

pub(crate) fn feature_tree_violations(output: &str, profile: &FeatureProfile) -> Vec<String> {
    let variant = &profile.name;
    let constraints = &profile.constraints;
    let mut features_by_package = BTreeMap::<String, BTreeSet<String>>::new();
    let mut package_identities = BTreeSet::new();
    let mut errors = Vec::new();
    for (line_number, raw_line) in output.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((package, raw_features)) = line.split_once('|') else {
            errors.push(format!(
                "{variant} Cargo feature tree 第 {} 行格式无效",
                line_number + 1
            ));
            continue;
        };
        let Some((name, _version)) = package.split_once(" v") else {
            errors.push(format!(
                "{variant} Cargo feature tree 第 {} 行格式无效",
                line_number + 1
            ));
            continue;
        };
        if package_name(name).is_none() {
            errors.push(format!(
                "{variant} Cargo feature tree 第 {} 行格式无效",
                line_number + 1
            ));
            continue;
        }
        package_identities.insert(package.trim_end_matches(" (*)").to_owned());
        let features = features_by_package.entry(name.to_owned()).or_default();
        features.extend(
            raw_features
                .trim_end_matches(" (*)")
                .split(',')
                .filter(|feature| !feature.is_empty())
                .map(str::to_owned),
        );
    }
    if package_identities.is_empty() {
        errors.push(format!("{variant} Cargo feature tree 没有有效 package"));
    }
    if let Some(maximum) = constraints.max_unique_packages
        && package_identities.len() > maximum
    {
        errors.push(format!(
            "{variant} 定向运行图 unique package closure 超限：{} > {maximum}",
            package_identities.len()
        ));
    }
    for package in &constraints.forbidden_packages {
        if features_by_package.contains_key(package) {
            errors.push(format!(
                "{variant} 定向运行图包含禁止的 {package} package/provider"
            ));
        }
    }
    for (package, forbidden) in &constraints.forbidden_features {
        let Some(actual) = features_by_package.get(package) else {
            continue;
        };
        let enabled = forbidden
            .iter()
            .filter(|feature| actual.contains(*feature))
            .cloned()
            .collect::<Vec<_>>();
        if !enabled.is_empty() {
            errors.push(format!(
                "{variant} 定向运行图启用了禁止的 {package} provider feature：{}",
                enabled.join(", ")
            ));
        }
    }
    for package in &constraints.required_packages {
        if !features_by_package.contains_key(package) {
            errors.push(format!("{variant} 定向运行图缺少 {package}"));
        }
    }
    for (package, required) in &constraints.required_features {
        let Some(actual) = features_by_package.get(package) else {
            errors.push(format!("{variant} 定向运行图缺少 {package}"));
            continue;
        };
        let missing = required
            .iter()
            .filter(|feature| !actual.contains(*feature))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            errors.push(format!(
                "{variant} 定向运行图的 {package} 缺少 AWS-LC feature：{}",
                missing.join(", ")
            ));
        }
    }
    errors
}

pub(crate) fn feature_tree_args(profile: &FeatureProfile) -> Vec<String> {
    let command = &profile.command;
    let mut arguments = vec![
        "tree".into(),
        "--locked".into(),
        "-p".into(),
        command.package.clone(),
    ];
    if command.no_default_features {
        arguments.push("--no-default-features".into());
    }
    if !command.features.is_empty() {
        arguments.push("--features".into());
        arguments.push(command.features.join(","));
    }
    arguments.extend([
        "--edges".into(),
        command.edges.join(","),
        "--prefix".into(),
        "none".into(),
        "--format".into(),
        "{p}|{f}".into(),
    ]);
    arguments
}

fn cargo_tree(root: &Path, arguments: &[&str]) -> std::result::Result<String, String> {
    let output = child_command("cargo")
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("无法启动 Cargo：{error}"))?;
    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map_err(|_| "cargo tree 输出不是 UTF-8".to_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let diagnostic = sanitize(&stderr);
    let code = output
        .status
        .code()
        .map_or_else(|| "signal".to_owned(), |code| code.to_string());
    Err(if diagnostic.is_empty() {
        format!("退出码 {code}")
    } else {
        format!("退出码 {code}：{diagnostic}")
    })
}

fn sanitize(value: &str) -> String {
    let value = Regex::new(r"(?i)\b(password|passwd|token|secret|authorization)\b\s*[:=]\s*\S+")
        .expect("固定敏感值正则")
        .replace_all(value, "$1=<已隐藏>");
    Regex::new(r"([A-Za-z][A-Za-z0-9+.-]*://)[^/@\s]+@")
        .expect("固定 URL 凭据正则")
        .replace_all(&value, "$1<已隐藏>@")
        .chars()
        .take(2_000)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn package_name(value: &str) -> Option<()> {
    Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_-]*$")
        .expect("固定 Cargo 包名正则")
        .is_match(value)
        .then_some(())
}
