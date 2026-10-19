use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Value, json};

use super::super::{digest, json as json_io, lifecycle::manifest};
use super::input::TargetContext;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

pub(crate) fn verify_initial_inventory(context: &TargetContext) -> Result<Value> {
    let binding = context
        .initialized
        .pointer("/inventory/receipt")
        .ok_or("目标初始化收据缺少 initial inventory")?;
    initial_inventory(
        &context.backend,
        &context.target_dir,
        binding,
        context.target_dir.join("failure.json").is_file(),
    )
}

pub(crate) fn initial_inventory(
    backend: &Path,
    target: &Path,
    binding: &Value,
    resumed: bool,
) -> Result<Value> {
    let receipt_path = bound_file(backend, binding, "初始库存收据")?;
    let parent = receipt_path.parent().ok_or("初始库存收据缺少父目录")?;
    let resumed_directory = parent.parent() == Some(target)
        && parent
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("inventory-resume-"));
    if receipt_path.file_name().and_then(|name| name.to_str()) != Some("inventory.json")
        || (parent != target.join("inventory-initial") && !(resumed && resumed_directory))
    {
        return Err("初始库存文件不属于本代次".into());
    }
    let receipt = json_io::read(&receipt_path)?;
    let inventories = receipt
        .get("inventories")
        .and_then(Value::as_object)
        .ok_or("初始库存收据缺少 inventories")?;
    for (name, expected) in inventories {
        if !safe_relative(name) || name.contains('/') {
            return Err("初始库存文件名越界".into());
        }
        let path = parent.join(name);
        validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
        json_io::exact(expected, &["bytes", "sha256"], "原始库存文件摘要")?;
        let actual = digest::file(&path)?;
        if actual.bytes
            != expected
                .get("bytes")
                .and_then(Value::as_u64)
                .unwrap_or_default()
            || actual.sha256
                != expected
                    .get("sha256")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
        {
            return Err("初始库存原始文件或摘要变化".into());
        }
    }
    Ok(receipt)
}

pub(crate) fn verify_history(context: &TargetContext) -> Result<()> {
    let expected = context
        .initialized
        .get("history")
        .and_then(Value::as_object)
        .ok_or("目标初始化收据缺少 history")?;
    let current = history(&context.backend, &context.target_dir)?;
    if current.as_object() != Some(expected) {
        return Err("目标初始化历史文件集合或摘要变化".into());
    }
    Ok(())
}

pub(crate) fn history(backend: &Path, target: &Path) -> Result<Value> {
    validate_local_test_path(target, backend, LocalTestPathKind::ExistingDirectory)?;
    let mut names = BTreeMap::new();
    for name in [
        "initialize.started.json",
        "prepare.json",
        "request.json",
        "reset-plan.json",
        "reset-plan-confirm.json",
    ] {
        add_required_file(&mut names, backend, target, name)?;
    }
    for stage in ["sentinel", "reset"] {
        for state in ["intent", "confirmed"] {
            add_required_file(
                &mut names,
                backend,
                target,
                &format!("{stage}.{state}.json"),
            )?;
        }
    }
    for key in ["shared-control", "shared", "dedicated-a", "dedicated-b"] {
        for state in ["intent", "confirmed"] {
            add_required_file(
                &mut names,
                backend,
                target,
                &format!("create-{key}.{state}.json"),
            )?;
        }
    }
    let patterns = [
        r"^failure[^/]*\.json$",
        r"^resume-prepare-[^/]*\.json$",
        r"^resume-initialize-[^/]*\.(intent|started|failure)\.json$",
        r"^resume-redis-markers-[^/]*\.json$",
        r"^resume-migrate-[^/]*\.json$",
        r"^resume-reset-plan[^/]*\.json$",
    ];
    let patterns = patterns
        .iter()
        .map(|pattern| Regex::new(pattern))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for entry in fs::read_dir(target)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "目标历史文件名必须为 UTF-8")?;
        if patterns.iter().any(|pattern| pattern.is_match(&name)) {
            add_required_file(&mut names, backend, target, &name)?;
        }
    }
    Ok(serde_json::to_value(names)?)
}

pub(crate) fn verify_reset(context: &TargetContext) -> Result<()> {
    let plan = json_io::read(&context.target_dir.join("reset-plan.json"))?;
    let manifest = plan.get("manifest").ok_or("reset-plan 缺少 manifest")?;
    let hash = plan["plan_hash"]
        .as_str()
        .ok_or("reset-plan 缺少 plan_hash")?;
    let observed = super::reset::completed(&context.backend, &context.target_dir, manifest, hash)?;
    if context.initialized.get("reset") != Some(&observed) {
        return Err("目标 reset 完成证据与初始化代次不同".into());
    }
    Ok(())
}
pub(crate) fn verify_generation(context: &TargetContext) -> Result<()> {
    let generation = context
        .initialized
        .get("generation")
        .ok_or("目标初始化收据缺少 generation")?;
    if generation.get("request_sha256").and_then(Value::as_str)
        != Some(digest::plan_hash(&context.request)?.as_str())
        || generation
            .get("configuration_sha256")
            .and_then(Value::as_str)
            != context
                .request
                .get("configuration_sha256")
                .and_then(Value::as_str)
        || context.initialized.get("scope_id").and_then(Value::as_str)
            != Some(context.target_scope()?)
        || context.initialized.get("id").and_then(Value::as_str)
            != context.request.get("id").and_then(Value::as_str)
    {
        return Err("目标 generation、请求或 scope 绑定不同".into());
    }
    let maintenance = generation
        .get("maintenance")
        .ok_or("目标 generation 缺少维护构建")?;
    let path = manifest::bound_file(
        &context.backend,
        context
            .request
            .get("maintenance_build")
            .ok_or("目标请求缺少维护构建")?,
        "目标维护构建",
        true,
    )?;
    let current = json_io::read(&path)?;
    if current != *maintenance {
        return Err("目标维护构建代次已变化".into());
    }
    super::super::maintenance::verify_evidence(
        &context.execution_backend()?,
        &path,
        Some(&current),
    )?;
    Ok(())
}

fn bound_file(root: &Path, value: &Value, label: &str) -> Result<PathBuf> {
    let path = manifest::bound_file(root, value, label, true)?;
    validate_local_test_path(&path, root, LocalTestPathKind::ExistingFile)
        .map_err(|error| format!("{label}无效：{error}"))?;
    Ok(path)
}

fn add_required_file(
    map: &mut BTreeMap<String, Value>,
    backend: &Path,
    root: &Path,
    name: &str,
) -> Result<()> {
    let path = root.join(name);
    validate_local_test_path(&path, backend, LocalTestPathKind::ExistingFile)?;
    let actual = digest::file(&path)?;
    map.insert(
        name.to_owned(),
        json!({"bytes": actual.bytes, "sha256": actual.sha256}),
    );
    Ok(())
}

fn safe_relative(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', ':'])
        && !value.starts_with('/')
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}
