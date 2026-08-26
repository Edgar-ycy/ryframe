use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use crate::{Result, process::command_output};

use super::model::{BackendSnapshotProfile, FrontendProfile, VerifySelection, WorkspaceGraph};

pub(crate) fn changed_paths(repository: &Path) -> Result<Vec<String>> {
    if !repository.is_dir() {
        return Err(format!("Git 工作树不存在：{}", repository.display()).into());
    }
    let tracked = command_output(
        repository,
        "git",
        &["diff", "--name-only", "-z", "HEAD", "--"],
    )?;
    let untracked = command_output(
        repository,
        "git",
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut paths = tracked
        .split('\0')
        .chain(untracked.split('\0'))
        .filter(|path| !path.is_empty())
        .map(normalize_relative_path)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

pub(crate) fn changed_paths_between(
    repository: &Path,
    base: &str,
    head: &str,
) -> Result<Vec<String>> {
    if !repository.is_dir() {
        return Err(format!("Git 工作树不存在：{}", repository.display()).into());
    }
    if base.trim().is_empty() || head.trim().is_empty() {
        return Err("CI 变更范围缺少 base 或 head".into());
    }
    let output = command_output(
        repository,
        "git",
        &["diff", "--name-only", "-z", base, head, "--"],
    )?;
    Ok(output
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(normalize_relative_path)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

fn normalize_relative_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub(crate) fn classify_changes(
    backend_paths: &[String],
    frontend_paths: &[String],
    graph: &WorkspaceGraph,
) -> VerifySelection {
    let mut selection = VerifySelection::default();
    for path in backend_paths {
        classify_backend_path(path, graph, &mut selection);
        if selection.full_reason.is_some() {
            return selection;
        }
    }
    for path in frontend_paths {
        classify_frontend_path(path, &mut selection);
        if selection.full_reason.is_some() {
            return selection;
        }
    }
    selection
}

pub(crate) fn complete_verify_selection(selection: &mut VerifySelection, graph: &WorkspaceGraph) {
    selection.backend_packages =
        reverse_dependency_closure(&selection.backend_packages, &graph.reverse_dependencies);
    if selection.backend_packages.contains("ryframe-api") {
        selection
            .backend_snapshot_profiles
            .insert(BackendSnapshotProfile::OpenApiContract);
    }
    if selection.backend_packages.contains("ryframe-db") {
        selection
            .backend_snapshot_profiles
            .insert(BackendSnapshotProfile::Mysql);
    }
}

pub(crate) fn needs_consumer_contract(profiles: &BTreeSet<BackendSnapshotProfile>) -> bool {
    profiles.contains(&BackendSnapshotProfile::OpenApiContract)
}

fn classify_backend_path(path: &str, graph: &WorkspaceGraph, selection: &mut VerifySelection) {
    if is_documentation(path) {
        selection.reasons.push(format!("后端文档：{path}"));
        return;
    }
    if path == "openapi/openapi.json" {
        selection
            .backend_snapshot_profiles
            .insert(BackendSnapshotProfile::OpenApiContract);
        selection.reasons.push(format!("后端 OpenAPI 快照：{path}"));
        return;
    }
    if path == "sql/ryframe_config.sql" {
        selection
            .backend_snapshot_profiles
            .insert(BackendSnapshotProfile::Mysql);
        selection.reasons.push(format!("后端 MySQL 快照：{path}"));
        return;
    }
    if is_backend_shared_path(path) {
        selection.full_reason = Some(format!("后端共享、依赖、CI 或工具链文件发生变化：{path}"));
        return;
    }
    let package = graph
        .package_by_dir
        .iter()
        .filter(|(directory, _)| {
            path == directory.as_str() || path.starts_with(&format!("{directory}/"))
        })
        .max_by_key(|(directory, _)| directory.len())
        .map(|(_, package)| package);
    if let Some(package) = package {
        if path.ends_with("/Cargo.toml") || path.ends_with("/build.rs") {
            selection.full_reason = Some(format!("包依赖或构建配置发生变化：{path}"));
        } else {
            selection.backend_packages.insert(package.clone());
            selection.reasons.push(format!("后端包 {package}：{path}"));
        }
        return;
    }
    selection.full_reason = Some(format!("无法安全分类后端变更：{path}"));
}

fn classify_frontend_path(path: &str, selection: &mut VerifySelection) {
    if is_documentation(path) {
        selection.reasons.push(format!("前端文档：{path}"));
        return;
    }
    if is_frontend_shared_path(path) {
        selection.full_reason = Some(format!("前端共享、依赖、CI 或工具链文件发生变化：{path}"));
        return;
    }
    if path.starts_with("openapi/") || path.starts_with("src/api/generated/") {
        selection
            .frontend_profiles
            .insert(FrontendProfile::Contract);
        selection.reasons.push(format!("前端契约：{path}"));
    } else if path.starts_with("tests/e2e/") || path.starts_with("tests/browser/") {
        selection.frontend_profiles.insert(FrontendProfile::Browser);
        selection.reasons.push(format!("浏览器测试：{path}"));
    } else if path.starts_with("src/")
        || path.starts_with("tests/unit/")
        || path.starts_with("public/")
    {
        selection.frontend_profiles.insert(FrontendProfile::Code);
        selection.reasons.push(format!("前端代码：{path}"));
    } else {
        selection.full_reason = Some(format!("无法安全分类前端变更：{path}"));
    }
}

fn is_documentation(path: &str) -> bool {
    matches!(
        path,
        "README.md"
            | "ARCHITECTURE.md"
            | "LICENSE"
            | "LICENSE.md"
            | "CHANGELOG.md"
            | "CONTRIBUTING.md"
            | "SECURITY.md"
            | "CODE_OF_CONDUCT.md"
            | "docs/api.md"
            | "docs/architecture.md"
            | "docs/data.md"
            | "docs/development.md"
            | "docs/operations.md"
    )
}

fn is_backend_shared_path(path: &str) -> bool {
    path == "Cargo.toml"
        || path == "Cargo.lock"
        || path.starts_with(".cargo/")
        || path.starts_with(".github/")
        || path.starts_with("catalog/")
        || path.starts_with("config/")
        || path.starts_with("openapi/")
        || path.starts_with("scripts/")
        || path.starts_with("sql/")
        || path.starts_with("xtask/")
        || path.starts_with("rust-toolchain")
        || matches!(path, ".gitignore" | "deny.toml")
}

fn is_frontend_shared_path(path: &str) -> bool {
    path == "package.json"
        || path == "pnpm-lock.yaml"
        || path == "openapi/source.json"
        || path.starts_with(".github/")
        || path.starts_with("scripts/")
        || path.starts_with("tsconfig")
        || path.contains(".config.")
        || matches!(path, ".gitignore" | "eslint.config.js")
}

pub(crate) fn reverse_dependency_closure(
    initial: &BTreeSet<String>,
    reverse_dependencies: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut selected = initial.clone();
    let mut queue = initial.iter().cloned().collect::<VecDeque<_>>();
    while let Some(package) = queue.pop_front() {
        if let Some(dependents) = reverse_dependencies.get(&package) {
            for dependent in dependents {
                if selected.insert(dependent.clone()) {
                    queue.push_back(dependent.clone());
                }
            }
        }
    }
    selected
}

pub(super) fn print_selection(selection: &VerifySelection) {
    for reason in &selection.reasons {
        println!("变更分类：{reason}");
    }
    if !selection.backend_packages.is_empty() {
        println!(
            "后端检查包（含反向依赖）：{}",
            selection
                .backend_packages
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !selection.backend_snapshot_profiles.is_empty() {
        let labels = selection
            .backend_snapshot_profiles
            .iter()
            .map(|profile| match profile {
                BackendSnapshotProfile::OpenApiContract => "OpenAPI 与消费契约",
                BackendSnapshotProfile::Mysql => "MySQL 基线",
            })
            .collect::<Vec<_>>();
        println!("后端快照画像：{}", labels.join("、"));
    }
    if !selection.frontend_profiles.is_empty() {
        let labels = selection
            .frontend_profiles
            .iter()
            .map(|profile| match profile {
                FrontendProfile::Contract => "契约",
                FrontendProfile::Code => "代码",
                FrontendProfile::Browser => "浏览器",
            })
            .collect::<Vec<_>>();
        println!("前端检查画像：{}", labels.join("、"));
    }
}

pub(crate) fn frontend_profile_commands(
    profiles: &BTreeSet<FrontendProfile>,
    consumer_contract_ran: bool,
) -> Vec<&'static str> {
    let contract = profiles.contains(&FrontendProfile::Contract);
    let code = profiles.contains(&FrontendProfile::Code);
    let mut commands = Vec::new();
    if contract && !consumer_contract_ran {
        // api:check 同时覆盖来源摘要、契约结构、派生物与 operation 使用，避免拆分后漏项。
        commands.push("api:check");
    }
    if code {
        commands.extend(["check:source-size", "lint", "lint:styles"]);
    }
    if contract || code {
        if code && !contract && !consumer_contract_ran {
            commands.push("check:api-operations");
        }
        if !consumer_contract_ran {
            commands.extend(["typecheck", "test:unit"]);
        }
        commands.push("build");
    }
    if code {
        commands.push("check:bundle");
    }
    if profiles.contains(&FrontendProfile::Browser) {
        commands.push("test:browser-smoke");
    }
    commands
}

pub(super) fn load_workspace_metadata(root: &Path) -> Result<serde_json::Value> {
    let output = command_output(
        root,
        "cargo",
        &["metadata", "--format-version", "1", "--no-deps"],
    )?;
    serde_json::from_str(&output)
        .map_err(|error| format!("Cargo 工作区元数据不是有效 JSON：{error}").into())
}

pub(crate) fn load_workspace_graph(root: &Path) -> Result<WorkspaceGraph> {
    let metadata = load_workspace_metadata(root)?;
    let members = metadata
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 workspace_members")?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<BTreeSet<_>>();
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("Cargo 元数据缺少 packages")?;
    let mut package_by_dir = BTreeMap::new();
    let mut dependency_names = BTreeMap::<String, BTreeSet<String>>::new();
    let mut workspace_names = BTreeSet::new();

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
            .ok_or("Cargo package 缺少 name")?
            .to_owned();
        let manifest = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .ok_or("Cargo package 缺少 manifest_path")?;
        let directory = PathBuf::from(manifest)
            .parent()
            .ok_or("Cargo manifest_path 没有父目录")?
            .strip_prefix(root)
            .map_err(|_| format!("Cargo 包 {name} 不属于当前工作区"))?
            .to_string_lossy()
            .replace('\\', "/");
        if package_by_dir.insert(directory, name.clone()).is_some() {
            return Err(format!("Cargo 工作区存在重复包目录：{name}").into());
        }
        workspace_names.insert(name.clone());
        let dependencies = package
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .ok_or("Cargo package 缺少 dependencies")?
            .iter()
            .filter_map(|dependency| dependency.get("name"))
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
        dependency_names.insert(name, dependencies);
    }

    let mut reverse_dependencies = BTreeMap::<String, BTreeSet<String>>::new();
    for (package, dependencies) in dependency_names {
        for dependency in dependencies {
            if workspace_names.contains(&dependency) {
                reverse_dependencies
                    .entry(dependency)
                    .or_default()
                    .insert(package.clone());
            }
        }
    }
    Ok(WorkspaceGraph {
        package_by_dir,
        reverse_dependencies,
    })
}
