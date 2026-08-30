use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde::Serialize;

use crate::check::{WorkspaceGraph, reverse_dependency_closure};

pub(crate) const OWNERSHIP_PATH: &str = "catalog/resources/.ownership.toml";
const RESOURCE_PREFIX: &str = "catalog/resources/";
const MAX_SOURCE_RESOURCES: usize = 8;
const MAX_OWNED_OUTPUTS: usize = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangedFile {
    pub(crate) status: ChangeStatus,
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResourceDefinition {
    pub(crate) name: String,
    pub(crate) relations: BTreeSet<String>,
    pub(crate) document: toml::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnershipEntry {
    pub(crate) resource: String,
    pub(crate) root: String,
    pub(crate) path: String,
    pub(crate) fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnershipManifest {
    pub(crate) format_version: u16,
    pub(crate) generator_version: String,
    pub(crate) entries: Vec<OwnershipEntry>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResourceGateInput {
    pub(crate) changes: Vec<ChangedFile>,
    pub(crate) base_resources: BTreeMap<String, ResourceDefinition>,
    pub(crate) head_resources: BTreeMap<String, ResourceDefinition>,
    pub(crate) base_ownership: OwnershipManifest,
    pub(crate) head_ownership: OwnershipManifest,
    pub(crate) workspace_graph: WorkspaceGraph,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResourceChangeSet {
    pub(crate) source_resources: BTreeSet<String>,
    pub(crate) impacted_resources: BTreeSet<String>,
    pub(crate) owned_backend_paths: BTreeSet<String>,
    pub(crate) owned_frontend_paths: BTreeSet<String>,
    pub(crate) affected_crates: BTreeSet<String>,
    pub(crate) schema_changed: bool,
    pub(crate) permissions_changed: bool,
    pub(crate) relations_changed: bool,
    pub(crate) ambiguous_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateStep {
    FullRustGate,
    FullIntegration,
    FullConsumerContract,
    ResourceDrift,
    ResourceWorkspace,
    AffectedClippy(BTreeSet<String>),
    AffectedTest(BTreeSet<String>),
    PermissionContract,
    MigrationContract,
    OpenApiAndFrontendConsumer,
}

pub(crate) fn should_run_for_paths(paths: &[String]) -> bool {
    paths.iter().any(|path| {
        is_resource_source(path)
            || path == OWNERSHIP_PATH
            || is_expected_snapshot(path)
            || looks_generated(path)
            || full_invalidation_path(path).is_some()
    })
}

pub(crate) fn delegates_generic_ci_path(path: &str) -> bool {
    full_invalidation_path(path).is_none()
        && (is_resource_source(path)
            || path == OWNERSHIP_PATH
            || is_expected_snapshot(path)
            || looks_generated(path))
}

pub(crate) fn full_fallback_reason_for_paths(paths: &[String]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| full_invalidation_path(path).map(|reason| format!("{reason}：{path}")))
}

pub(crate) fn plan_steps(change_set: &ResourceChangeSet) -> Vec<GateStep> {
    if change_set.ambiguous_reason.is_some() {
        return vec![
            GateStep::ResourceDrift,
            GateStep::FullRustGate,
            GateStep::FullIntegration,
            GateStep::PermissionContract,
            GateStep::MigrationContract,
            GateStep::FullConsumerContract,
        ];
    }
    vec![
        GateStep::ResourceDrift,
        GateStep::ResourceWorkspace,
        GateStep::AffectedClippy(change_set.affected_crates.clone()),
        GateStep::AffectedTest(change_set.affected_crates.clone()),
        GateStep::PermissionContract,
        GateStep::MigrationContract,
        GateStep::OpenApiAndFrontendConsumer,
    ]
}

pub(crate) fn analyze(input: &ResourceGateInput) -> ResourceChangeSet {
    let mut result = ResourceChangeSet::default();
    if let Some(reason) = structural_fallback_reason(input) {
        result.ambiguous_reason = Some(reason);
        return result;
    }

    let base_owners = match ownership_index(&input.base_ownership) {
        Ok(index) => index,
        Err(reason) => return ambiguous(result, reason),
    };
    let head_owners = match ownership_index(&input.head_ownership) {
        Ok(index) => index,
        Err(reason) => return ambiguous(result, reason),
    };
    if let Some(reason) = collect_source_changes(input, &head_owners, &mut result) {
        return ambiguous(result, reason);
    }
    if let Some(reason) = collect_ownership_changes(&base_owners, &head_owners, &mut result) {
        return ambiguous(result, reason);
    }
    if result.source_resources.is_empty() {
        return ambiguous(result, "差异中没有可归属的资源事实源".to_owned());
    }
    if result.source_resources.len() > MAX_SOURCE_RESOURCES {
        let count = result.source_resources.len();
        return ambiguous(
            result,
            format!("资源事实源数量超过 {MAX_SOURCE_RESOURCES}：{count}"),
        );
    }

    let relationships = relationship_graph(&input.base_resources, &input.head_resources);
    result.impacted_resources = relationship_closure(&result.source_resources, &relationships);
    if result.impacted_resources.len() > MAX_SOURCE_RESOURCES {
        let count = result.impacted_resources.len();
        return ambiguous(
            result,
            format!("关系影响后的资源数量超过 {MAX_SOURCE_RESOURCES}：{count}"),
        );
    }
    let total_resources = resource_names(&input.head_resources).len();
    if total_resources == 0 || result.impacted_resources.len() * 2 > total_resources {
        let closure_count = result.impacted_resources.len();
        return ambiguous(
            result,
            format!(
                "关系影响闭包超过资源总数的 50%：{}/{}",
                closure_count, total_resources
            ),
        );
    }
    if let Some(reason) = collect_owned_paths(input, &mut result) {
        return ambiguous(result, reason);
    }
    let output_count = result.owned_backend_paths.len() + result.owned_frontend_paths.len();
    if output_count > MAX_OWNED_OUTPUTS {
        return ambiguous(
            result,
            format!("受管输出数量超过 {MAX_OWNED_OUTPUTS}：{output_count}"),
        );
    }
    if let Some(reason) = collect_affected_crates(input, &mut result) {
        return ambiguous(result, reason);
    }
    result
}

fn structural_fallback_reason(input: &ResourceGateInput) -> Option<String> {
    if input.base_ownership.format_version != 1 || input.head_ownership.format_version != 1 {
        return Some("ownership format_version 不是受支持的 1".to_owned());
    }
    if input.base_ownership.format_version != input.head_ownership.format_version
        || input.base_ownership.generator_version != input.head_ownership.generator_version
    {
        return Some("ownership schema 或生成器版本发生变化".to_owned());
    }
    for change in &input.changes {
        if matches!(change.status, ChangeStatus::Deleted | ChangeStatus::Renamed) {
            let path = change.old_path.as_deref().map_or_else(
                || change.path.clone(),
                |old_path| format!("{old_path} -> {}", change.path),
            );
            return Some(format!("删除或重命名无法安全缩小范围：{path}"));
        }
        if let Some(reason) = full_invalidation_path(&change.path) {
            return Some(format!("{reason}：{}", change.path));
        }
    }
    None
}

fn collect_source_changes(
    input: &ResourceGateInput,
    head_owners: &BTreeMap<(String, String), OwnershipEntry>,
    result: &mut ResourceChangeSet,
) -> Option<String> {
    let mut catalog_output_changed = false;
    for change in &input.changes {
        let path = change.path.as_str();
        if path == OWNERSHIP_PATH {
            continue;
        }
        if is_resource_source(path) {
            let Some(head) = input.head_resources.get(path) else {
                return Some(format!("HEAD 缺少已变更资源清单：{path}"));
            };
            if contains_forbidden_sql(&head.document) {
                return Some(format!("资源清单包含禁止的 SQL DSL：{path}"));
            }
            if let Some(base) = input.base_resources.get(path) {
                if base.name != head.name {
                    return Some(format!(
                        "资源标识发生重命名：{} -> {}",
                        base.name, head.name
                    ));
                }
                result.schema_changed |=
                    section_changed(base, head, &["storage", "database", "fields"]);
                result.permissions_changed |=
                    section_changed(base, head, &["api", "access", "menu", "route"]);
                result.relations_changed |= section_changed(base, head, &["relations"]);
            } else {
                result.schema_changed = true;
                result.permissions_changed = true;
                result.relations_changed |= !head.relations.is_empty();
            }
            result.source_resources.insert(head.name.clone());
            continue;
        }
        if is_ignored_document(path) || is_expected_snapshot(path) {
            continue;
        }
        if let Some(owner) = head_owners.get(&("backend".to_owned(), path.to_owned())) {
            if owner.resource == "__catalog__" {
                catalog_output_changed = true;
            } else {
                result.source_resources.insert(owner.resource.clone());
            }
            continue;
        }
        if looks_generated(path) {
            return Some(format!("生成文件无法由 ownership 解释：{path}"));
        }
        return Some(format!("资源门禁无法安全分类变更：{path}"));
    }
    if catalog_output_changed && result.source_resources.is_empty() {
        return Some("只有聚合生成输出发生变化，无法确定源资源".to_owned());
    }
    None
}

fn collect_ownership_changes(
    base: &BTreeMap<(String, String), OwnershipEntry>,
    head: &BTreeMap<(String, String), OwnershipEntry>,
    result: &mut ResourceChangeSet,
) -> Option<String> {
    for key in base.keys().chain(head.keys()).collect::<BTreeSet<_>>() {
        match (base.get(key), head.get(key)) {
            (Some(old), Some(new)) if old.resource != new.resource => {
                return Some(format!("ownership 路径归属发生变化：{}/{}", key.0, key.1));
            }
            (Some(old), Some(new)) if old != new && new.resource != "__catalog__" => {
                result.source_resources.insert(new.resource.clone());
            }
            (None, Some(new)) if new.resource != "__catalog__" => {
                result.source_resources.insert(new.resource.clone());
            }
            (Some(old), None) if old.resource != "__catalog__" => {
                result.source_resources.insert(old.resource.clone());
            }
            _ => {}
        }
    }
    None
}

fn collect_owned_paths(
    input: &ResourceGateInput,
    result: &mut ResourceChangeSet,
) -> Option<String> {
    let base_owners = match ownership_index(&input.base_ownership) {
        Ok(index) => index,
        Err(reason) => return Some(reason),
    };
    for entry in &input.head_ownership.entries {
        if entry.resource != "__catalog__" && !result.impacted_resources.contains(&entry.resource) {
            continue;
        }
        if entry.resource == "__catalog__"
            && base_owners
                .get(&(entry.root.clone(), entry.path.clone()))
                .is_some_and(|base| base.fingerprint == entry.fingerprint)
        {
            continue;
        }
        match entry.root.as_str() {
            "backend" => {
                result.owned_backend_paths.insert(entry.path.clone());
            }
            "frontend" => {
                result.owned_frontend_paths.insert(entry.path.clone());
            }
            root => return Some(format!("ownership 包含未知 root：{root}")),
        }
    }
    None
}

fn collect_affected_crates(
    input: &ResourceGateInput,
    result: &mut ResourceChangeSet,
) -> Option<String> {
    let mut direct = BTreeSet::new();
    for path in &result.owned_backend_paths {
        let package = input
            .workspace_graph
            .package_by_dir
            .iter()
            .filter(|(directory, _)| {
                path == *directory || path.starts_with(&format!("{directory}/"))
            })
            .max_by_key(|(directory, _)| directory.len())
            .map(|(_, package)| package.clone());
        if let Some(package) = package {
            direct.insert(package);
        } else if path.starts_with("crates/") {
            return Some(format!("受管后端输出不属于 Cargo Workspace 包：{path}"));
        }
    }
    if direct.is_empty() {
        return Some("资源没有可编译的受影响后端 crate".to_owned());
    }
    result.affected_crates =
        reverse_dependency_closure(&direct, &input.workspace_graph.reverse_dependencies);
    let workspace = input
        .workspace_graph
        .package_by_dir
        .values()
        .cloned()
        .collect::<BTreeSet<_>>();
    if !workspace.is_empty() && result.affected_crates == workspace {
        return Some("受影响 crate 的反向依赖闭包等于整个 Workspace".to_owned());
    }
    None
}

fn ownership_index(
    manifest: &OwnershipManifest,
) -> Result<BTreeMap<(String, String), OwnershipEntry>, String> {
    let mut index = BTreeMap::new();
    for entry in &manifest.entries {
        if entry.root != "backend" && entry.root != "frontend" {
            return Err(format!("ownership 包含未知 root：{}", entry.root));
        }
        let key = (entry.root.clone(), entry.path.clone());
        if index.insert(key, entry.clone()).is_some() {
            return Err(format!(
                "ownership 包含重复路径：{}/{}",
                entry.root, entry.path
            ));
        }
    }
    Ok(index)
}

fn relationship_graph(
    base: &BTreeMap<String, ResourceDefinition>,
    head: &BTreeMap<String, ResourceDefinition>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut graph = BTreeMap::<String, BTreeSet<String>>::new();
    for resource in base.values().chain(head.values()) {
        graph.entry(resource.name.clone()).or_default();
        for target in &resource.relations {
            graph
                .entry(resource.name.clone())
                .or_default()
                .insert(target.clone());
            graph
                .entry(target.clone())
                .or_default()
                .insert(resource.name.clone());
        }
    }
    graph
}

fn relationship_closure(
    initial: &BTreeSet<String>,
    graph: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut selected = initial.clone();
    let mut queue = initial.iter().cloned().collect::<VecDeque<_>>();
    while let Some(resource) = queue.pop_front() {
        for related in graph.get(&resource).into_iter().flatten() {
            if selected.insert(related.clone()) {
                queue.push_back(related.clone());
            }
        }
    }
    selected
}

fn resource_names(resources: &BTreeMap<String, ResourceDefinition>) -> BTreeSet<String> {
    resources.values().map(|item| item.name.clone()).collect()
}

fn section_changed(base: &ResourceDefinition, head: &ResourceDefinition, keys: &[&str]) -> bool {
    keys.iter()
        .any(|key| base.document.get(*key) != head.document.get(*key))
}

fn contains_forbidden_sql(value: &toml::Value) -> bool {
    match value {
        toml::Value::Table(table) => table.iter().any(|(key, child)| {
            matches!(
                key.replace('-', "_").to_ascii_lowercase().as_str(),
                "sql" | "raw_sql" | "query_sql"
            ) || contains_forbidden_sql(child)
        }),
        toml::Value::Array(values) => values.iter().any(contains_forbidden_sql),
        _ => false,
    }
}

fn is_resource_source(path: &str) -> bool {
    path.starts_with(RESOURCE_PREFIX)
        && path.ends_with(".toml")
        && path != OWNERSHIP_PATH
        && !path[RESOURCE_PREFIX.len()..].contains('/')
}

fn full_invalidation_path(path: &str) -> Option<&'static str> {
    let cargo_or_build = path == "Cargo.toml"
        || path == "Cargo.lock"
        || path.ends_with("/Cargo.toml")
        || path == "build.rs"
        || path.ends_with("/build.rs")
        || path.starts_with("rust-toolchain");
    if cargo_or_build {
        return Some("Cargo、工具链或 build.rs 发生变化");
    }
    for (prefix, reason) in [
        (".cargo/", "Cargo 配置发生变化"),
        (".github/", "CI 定义发生变化"),
        ("architecture/", "架构策略发生变化"),
        ("crates/ryframe-generator/", "生成器或模板发生变化"),
        ("scripts/", "策略或构建检查器发生变化"),
        ("xtask/", "CI 编排实现发生变化"),
    ] {
        if path.starts_with(prefix) {
            return Some(reason);
        }
    }
    if path.starts_with(RESOURCE_PREFIX) && path != OWNERSHIP_PATH && !is_resource_source(path) {
        return Some("资源 schema 或目录结构发生变化");
    }
    (path == "catalog/access.toml").then_some("访问控制事实源发生变化")
}

fn is_expected_snapshot(path: &str) -> bool {
    matches!(
        path,
        "openapi/openapi.json" | "sql/ryframe_config.sql" | "catalog/migrations.lock.toml"
    )
}

fn is_ignored_document(path: &str) -> bool {
    path.ends_with(".md") || matches!(path, "LICENSE" | "LICENSE.md" | "CHANGELOG.md")
}

fn looks_generated(path: &str) -> bool {
    path.contains("/generated/")
        || path.contains(".generated.")
        || path.starts_with("src/generated/")
}

fn ambiguous(mut result: ResourceChangeSet, reason: String) -> ResourceChangeSet {
    result.ambiguous_reason = Some(reason);
    result
}
