//! 缓存重启只接受完整登记的源生产者和目标双角色均已退出。
use super::Native;
use crate::{
    Result,
    recovery::clone::native::{
        export, json as document, lifecycle::state, resources::common, target_verify,
    },
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub(super) fn stopped(runtime: &mut Native<'_>) -> Result<Value> {
    let backend = runtime.backend;
    let run = runtime.directory;
    if run.join("post-copy.json").try_exists()? || run.join("seed-runtime.json").try_exists()? {
        let saved = state::load(run, true)?;
        let current = state::attempts(&saved)?
            .last()
            .ok_or("已发布 seed 缺少当前缓存阶段")?;
        if current["stage"] != "cache-target" || current["status"] != "running" {
            return Err("已发布 seed 缓存操作必须为当前唯一持锁阶段".into());
        }
        return runtime.dispatch("restart-guard", json!({"number":current["number"]}));
    }
    let manifest = document::read(&run.join("manifest.json"))?;
    let source = common::read_bound(backend, &manifest["source_request"], "缓存来源请求")?;
    source_stopped(backend, run, &source)?;
    let initial = common::bound_file(backend, &manifest["initialized"], "缓存目标初始化")?;
    let (_, target) = match runtime.owner {
        Some(owner)
            if owner
                .protects(initial.parent().ok_or("缓存初始化缺少目录")?)
                .is_ok() =>
        {
            target_verify::published::owned_history(backend, &initial, owner)?
        }
        _ => target_verify::published::unlocked_history(backend, &initial)?,
    };
    let selected = target_verify::published::request_binding(backend, &target)?;
    target_stopped(backend, &target, &selected["selected"])?;
    common::bound_file(backend, &manifest["source_request"], "缓存来源请求后像")?;
    Ok(Value::Null)
}

fn source_stopped(backend: &Path, run: &Path, source: &Value) -> Result<()> {
    let mut processes = BTreeMap::new();
    for (role, descriptor) in document::object(&source["processes"], "源角色收据")? {
        let receipt = common::read_bound(backend, descriptor, "源角色收据")?;
        processes.insert(role.clone(), receipt["identity"].clone());
    }
    let registry = common::read_bound(backend, &source["producers_registry"], "源 producer 登记")?;
    let producers = export::proof::validate_registry(
        &registry,
        &source["source"]["scope_id"],
        document::text(&source["runtime"]["sha256"], "源 runtime 摘要")?,
        &processes,
    )?;
    let tool = std::env::current_exe()?;
    for producer in producers {
        crate::recovery::clone::native::seed::source_registration::quiet::require_stopped(
            backend,
            run,
            &producer["identity"],
            &tool,
        )?;
    }
    let runtime = common::read_bound(backend, &source["runtime"], "源 runtime")?;
    common::require_closed_runtime_ports(
        document::text(&source["source"]["api_url"], "源 API 地址")?,
        document::text(&runtime["worker_ready_url"], "源 Worker 就绪地址")?,
    )?;
    common::bound_file(
        backend,
        &source["producers_registry"],
        "源 producer 登记后像",
    )?;
    common::bound_file(backend, &source["runtime"], "源 runtime 后像")?;
    Ok(())
}

fn target_stopped(backend: &Path, request: &Value, selected: &Value) -> Result<()> {
    let directory = common::local_directory(
        backend,
        Path::new(document::text(
            &selected["runtime_dir"],
            "目标 runtime 目录",
        )?),
        "目标 runtime",
    )?;
    for role in ["api", "worker"] {
        let path = directory.join(format!("{role}.json"));
        if path.try_exists()? {
            let descriptor =
                common::binding(&common::common_file(backend, &path, "目标角色收据")?)?;
            let receipt = common::read_bound(backend, &descriptor, "目标角色收据")?;
            if receipt["scope_id"] != request["target"]["scope_id"] || receipt["role"] != role {
                return Err("目标运行收据角色或 scope 变化".into());
            }
            let identity = common::validate_process_identity(&receipt["identity"])?;
            let pid = u32::try_from(identity["pid"].as_u64().ok_or("目标角色 PID 无效")?)?;
            if common::process_identity(pid)?.is_some() {
                return Err("目标业务生产者仍在运行或身份不明".into());
            }
            common::bound_file(backend, &descriptor, "目标角色收据后像")?;
        }
    }
    common::require_closed_runtime_ports(
        document::text(&selected["api_url"], "目标 API 地址")?,
        document::text(&selected["worker_ready_url"], "目标 Worker 就绪地址")?,
    )
}
