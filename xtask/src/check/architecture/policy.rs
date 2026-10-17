use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use chrono::{Local, NaiveDate};
use toml::{Table, Value};

pub(crate) type Edge = (String, String);

#[derive(Debug, Clone, Default)]
pub(crate) struct Profile {
    pub(crate) packages: BTreeSet<String>,
    pub(crate) products: BTreeSet<String>,
    pub(crate) tools: BTreeSet<String>,
    pub(crate) allowed_edges: BTreeSet<Edge>,
    pub(crate) temporary_edges: BTreeSet<Edge>,
    pub(crate) expected_count: usize,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ArchitecturePolicy {
    pub(crate) active_profile: String,
    pub(crate) profiles: BTreeMap<String, Profile>,
    pub(crate) source_size: Table,
    pub(crate) test_layout: Table,
    pub(crate) function_size: Table,
}

pub(crate) fn load(root: &Path, errors: &mut Vec<String>) -> ArchitecturePolicy {
    let relative = "architecture/crate-boundaries.toml";
    let Ok(source) = std::fs::read_to_string(root.join(relative)) else {
        errors.push(format!("无法读取架构策略 {relative}"));
        return ArchitecturePolicy::default();
    };
    let Ok(Value::Table(policy)) = toml::from_str::<Value>(&source) else {
        errors.push(format!("无法读取架构策略 {relative}: TOML 解析失败"));
        return ArchitecturePolicy::default();
    };
    let mut result = ArchitecturePolicy::default();
    if policy.get("schema_version").and_then(Value::as_integer) != Some(2) {
        errors.push("架构策略 schema_version 必须为 2".into());
    }
    result.active_profile = policy
        .get("active_profile")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_owned();
    if result.active_profile.is_empty() {
        errors.push("架构策略 active_profile 必须是非空字符串".into());
    }
    match policy.get("profiles").and_then(Value::as_table) {
        Some(profiles) if !profiles.is_empty() => {
            for (name, profile) in profiles {
                result
                    .profiles
                    .insert(name.clone(), validate_profile(name, profile, errors));
            }
        }
        _ => errors.push("架构策略必须声明至少一个 profiles.*".into()),
    }
    if !result.profiles.contains_key(&result.active_profile) {
        errors.push(format!(
            "active_profile 未声明: {:?}",
            result.active_profile
        ));
    }
    result.source_size = required_table(&policy, "source_size", errors);
    result.test_layout = required_table(&policy, "test_layout", errors);
    result.function_size = required_table(&policy, "function_size", errors);
    result
}

fn required_table(policy: &Table, name: &str, errors: &mut Vec<String>) -> Table {
    match policy.get(name).and_then(Value::as_table) {
        Some(table) => table.clone(),
        None => {
            errors.push(format!("架构策略必须声明 {name}"));
            Table::new()
        }
    }
}

fn validate_profile(name: &str, profile: &Value, errors: &mut Vec<String>) -> Profile {
    let label = format!("profiles.{name}");
    let Some(table) = profile.as_table() else {
        errors.push(format!("{label} 必须是 TOML table"));
        return Profile::default();
    };
    let packages = parse_string_set(table.get("packages"), &format!("{label}.packages"), errors);
    let products = parse_string_set(
        table.get("product_packages"),
        &format!("{label}.product_packages"),
        errors,
    );
    let tools = parse_string_set(
        table.get("tool_packages"),
        &format!("{label}.tool_packages"),
        errors,
    );
    let allowed_edges = parse_edges(
        table.get("allowed_internal_edges"),
        &format!("{label}.allowed_internal_edges"),
        errors,
    );
    let temporary_edges = parse_temporary_edges(
        table.get("temporary_product_tool_edges"),
        &format!("{label}.temporary_product_tool_edges"),
        errors,
    );
    let expected_count = match table
        .get("expected_package_count")
        .and_then(Value::as_integer)
    {
        Some(value) if value > 0 => value as usize,
        _ => {
            errors.push(format!("{label}.expected_package_count 必须是正整数"));
            0
        }
    };
    if expected_count != 0 && expected_count != packages.len() {
        errors.push(format!(
            "{label} 声明 {expected_count} 个包，但 packages 实际包含 {} 个",
            packages.len()
        ));
    }
    if !products.is_disjoint(&tools) {
        errors.push(format!("{label} 的产品包与工具包不得重叠"));
    }
    let classified = products.union(&tools).cloned().collect::<BTreeSet<_>>();
    if classified != packages {
        let missing = packages
            .difference(&classified)
            .cloned()
            .collect::<Vec<_>>();
        let unknown = classified
            .difference(&packages)
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            errors.push(format!("{label} 未分类包: {}", missing.join(", ")));
        }
        if !unknown.is_empty() {
            errors.push(format!("{label} 分类了未声明包: {}", unknown.join(", ")));
        }
    }
    validate_allowed_edges(
        &label,
        &packages,
        &products,
        &tools,
        &allowed_edges,
        &temporary_edges,
        errors,
    );
    for component in cyclic_components(&packages, &allowed_edges) {
        errors.push(format!(
            "{label} 的允许依赖图存在 SCC: {}",
            component.join(" -> ")
        ));
    }
    Profile {
        packages,
        products,
        tools,
        allowed_edges,
        temporary_edges,
        expected_count,
    }
}

fn validate_allowed_edges(
    label: &str,
    packages: &BTreeSet<String>,
    products: &BTreeSet<String>,
    tools: &BTreeSet<String>,
    allowed: &BTreeSet<Edge>,
    temporary: &BTreeSet<Edge>,
    errors: &mut Vec<String>,
) {
    for (source, target) in allowed {
        if !packages.contains(source) || !packages.contains(target) {
            errors.push(format!(
                "{label} 的允许边引用未声明包: {source} -> {target}"
            ));
        }
    }
    let product_tool = allowed
        .iter()
        .filter(|(source, target)| products.contains(source) && tools.contains(target))
        .cloned()
        .collect::<BTreeSet<_>>();
    let unknown = temporary
        .difference(&product_tool)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !unknown.is_empty() {
        errors.push(format!(
            "{label} 的产品依赖工具豁免不是允许边: {}",
            format_edges(&unknown)
        ));
    }
    let unapproved = product_tool
        .difference(temporary)
        .cloned()
        .collect::<BTreeSet<_>>();
    if !unapproved.is_empty() {
        errors.push(format!(
            "{label} 存在未豁免的产品 -> 工具边: {}",
            format_edges(&unapproved)
        ));
    }
}

fn parse_string_set(
    value: Option<&Value>,
    label: &str,
    errors: &mut Vec<String>,
) -> BTreeSet<String> {
    let Some(Value::Array(values)) = value else {
        errors.push(format!("{label} 必须是字符串数组"));
        return BTreeSet::new();
    };
    let mut result = BTreeSet::new();
    for value in values {
        let Some(value) = value.as_str() else {
            errors.push(format!("{label} 必须是字符串数组"));
            return BTreeSet::new();
        };
        if !result.insert(value.to_owned()) {
            errors.push(format!("{label} 不得包含重复项"));
        }
    }
    result
}

fn parse_edges(value: Option<&Value>, label: &str, errors: &mut Vec<String>) -> BTreeSet<Edge> {
    parse_string_set(value, label, errors)
        .into_iter()
        .filter_map(|raw| parse_edge(&raw, label, errors))
        .collect()
}

fn parse_edge(value: &str, label: &str, errors: &mut Vec<String>) -> Option<Edge> {
    let parts = value.split("->").map(str::trim).collect::<Vec<_>>();
    if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
        errors.push(format!("{label} 包含无效依赖边: {value:?}"));
        return None;
    }
    Some((parts[0].to_owned(), parts[1].to_owned()))
}

pub(crate) fn parse_temporary_edges(
    value: Option<&Value>,
    label: &str,
    errors: &mut Vec<String>,
) -> BTreeSet<Edge> {
    let empty = Value::Array(Vec::new());
    let value = value.unwrap_or(&empty);
    let Value::Array(items) = value else {
        errors.push(format!("{label} 必须是 table 数组"));
        return BTreeSet::new();
    };
    let mut result = BTreeSet::new();
    for (index, item) in items.iter().enumerate() {
        let item_label = format!("{label}[{index}]");
        let Some(item) = item.as_table() else {
            errors.push(format!("{item_label} 必须是 table"));
            continue;
        };
        let allowed = ["edge", "reason", "expires"];
        let unknown = item
            .keys()
            .filter(|key| !allowed.contains(&key.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let missing = allowed
            .iter()
            .filter(|key| !item.contains_key(**key))
            .copied()
            .collect::<Vec<_>>();
        if !unknown.is_empty() {
            errors.push(format!("{item_label} 包含未知字段: {}", unknown.join(", ")));
        }
        if !missing.is_empty() {
            errors.push(format!("{item_label} 缺少字段: {}", missing.join(", ")));
            continue;
        }
        let Some(edge_value) = item.get("edge").and_then(Value::as_str) else {
            errors.push(format!("{item_label}.edge 必须是字符串"));
            continue;
        };
        let edge = parse_edge(edge_value, &format!("{item_label}.edge"), errors);
        if !item
            .get("reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| !reason.trim().is_empty())
        {
            errors.push(format!("{item_label}.reason 必须是非空字符串"));
        }
        validate_expiry(item.get("expires"), &item_label, errors);
        if let Some(edge) = edge
            && !result.insert(edge)
        {
            errors.push(format!("{label} 不得包含重复边: {edge_value}"));
        }
    }
    result
}

fn validate_expiry(value: Option<&Value>, label: &str, errors: &mut Vec<String>) {
    let expiry = match value {
        Some(Value::Datetime(datetime)) if datetime.time.is_none() => {
            datetime.date.and_then(|date| {
                NaiveDate::from_ymd_opt(date.year.into(), date.month.into(), date.day.into())
            })
        }
        Some(Value::String(value)) => NaiveDate::parse_from_str(value, "%Y-%m-%d").ok(),
        _ => None,
    };
    let Some(expiry) = expiry else {
        errors.push(format!("{label}.expires 必须是 YYYY-MM-DD 日期"));
        return;
    };
    if expiry < Local::now().date_naive() {
        errors.push(format!("{label} 已于 {} 过期", expiry.format("%Y-%m-%d")));
    }
}

pub(super) fn cyclic_components(
    nodes: &BTreeSet<String>,
    edges: &BTreeSet<Edge>,
) -> Vec<Vec<String>> {
    let mut graph = nodes
        .iter()
        .chain(edges.iter().flat_map(|(source, target)| [source, target]))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|node| (node, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for (source, target) in edges {
        graph
            .entry(source.clone())
            .or_default()
            .push(target.clone());
        graph.entry(target.clone()).or_default();
    }
    for targets in graph.values_mut() {
        targets.sort();
    }
    let mut finishing = Vec::new();
    let mut seen = BTreeSet::new();
    for node in graph.keys().cloned().collect::<Vec<_>>() {
        visit_finish(&graph, &node, &mut seen, &mut finishing);
    }
    let mut reverse = graph
        .keys()
        .cloned()
        .map(|node| (node, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for (source, targets) in &graph {
        for target in targets {
            reverse
                .entry(target.clone())
                .or_default()
                .push(source.clone());
        }
    }
    let self_loops = edges
        .iter()
        .filter(|(source, target)| source == target)
        .map(|(source, _)| source.clone())
        .collect::<BTreeSet<_>>();
    let mut components = Vec::new();
    seen.clear();
    for node in finishing.into_iter().rev() {
        if !seen.insert(node.clone()) {
            continue;
        }
        let mut component = Vec::new();
        collect_component(&reverse, &node, &mut seen, &mut component);
        component.sort();
        if component.len() > 1 || self_loops.contains(&component[0]) {
            components.push(component);
        }
    }
    components
}

fn visit_finish(
    graph: &BTreeMap<String, Vec<String>>,
    start: &str,
    seen: &mut BTreeSet<String>,
    finishing: &mut Vec<String>,
) {
    let mut stack = vec![(start.to_owned(), false)];
    while let Some((node, expanded)) = stack.pop() {
        if expanded {
            finishing.push(node);
            continue;
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        stack.push((node.clone(), true));
        for target in graph.get(&node).into_iter().flatten().rev() {
            if !seen.contains(target) {
                stack.push((target.clone(), false));
            }
        }
    }
}

fn collect_component(
    graph: &BTreeMap<String, Vec<String>>,
    start: &str,
    seen: &mut BTreeSet<String>,
    component: &mut Vec<String>,
) {
    let mut stack = vec![start.to_owned()];
    while let Some(node) = stack.pop() {
        component.push(node.clone());
        for target in graph.get(&node).into_iter().flatten() {
            if seen.insert(target.clone()) {
                stack.push(target.clone());
            }
        }
    }
}

pub(super) fn format_edges(edges: &BTreeSet<Edge>) -> String {
    edges
        .iter()
        .map(|(source, target)| format!("{source} -> {target}"))
        .collect::<Vec<_>>()
        .join(", ")
}
