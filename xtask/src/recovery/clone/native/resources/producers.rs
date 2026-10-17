use std::path::Path;

use serde_json::Value;

use super::{super::lifecycle::state, common, origin};
use crate::Result;

pub(crate) fn stopped(backend: &Path, run_dir: &Path, manifest: &Value) -> Result<()> {
    let published = ["post-copy.json", "seed-runtime.json"]
        .iter()
        .any(|name| run_dir.join(name).exists());
    if published {
        let value = state::load(run_dir, false)?;
        let running =
            state::running_attempt(&value)?.ok_or("已发布 seed 的资源操作缺少当前持锁阶段")?;
        let shape = (
            running.get("stage").and_then(Value::as_str),
            running.get("mode").and_then(Value::as_str),
            running.get("status").and_then(Value::as_str),
        );
        if !matches!(
            shape,
            (
                Some("storage-target"),
                Some("restart" | "stop" | "recover"),
                Some("running")
            ) | (
                Some("cache-target"),
                Some("restart" | "stop" | "recover" | "resume" | "reconcile"),
                Some("running")
            )
        ) {
            return Err("已发布 seed 只允许当前持锁的资源操作".into());
        }
        return target_stopped(backend, manifest);
    }
    source_stopped(backend, manifest)?;
    target_stopped(backend, manifest)
}

fn source_stopped(backend: &Path, manifest: &Value) -> Result<()> {
    let source = common::read_bound(
        backend,
        manifest
            .get("source_request")
            .ok_or("复制清单缺少来源请求")?,
        "来源请求",
    )?;
    let processes = source
        .get("processes")
        .and_then(Value::as_object)
        .ok_or("来源请求缺少生产者收据")?;
    for descriptor in processes.values() {
        let receipt = common::read_bound(backend, descriptor, "来源生产者收据")?;
        common::require_recorded_stopped(receipt.get("identity").ok_or("来源生产者收据缺少身份")?)?;
    }
    let runtime = common::read_bound(
        backend,
        source.get("runtime").ok_or("来源请求缺少运行收据")?,
        "来源运行收据",
    )?;
    let api = source
        .pointer("/source/api_url")
        .and_then(Value::as_str)
        .ok_or("来源请求缺少 API 地址")?;
    let worker = runtime
        .get("worker_ready_url")
        .and_then(Value::as_str)
        .ok_or("来源运行收据缺少 Worker 就绪地址")?;
    common::require_closed_port(api)?;
    common::require_closed_port(worker)
}

fn target_stopped(backend: &Path, manifest: &Value) -> Result<()> {
    let initialized = origin::initialized(
        backend,
        manifest
            .get("initialized")
            .ok_or("复制清单缺少目标初始化收据")?,
    )?;
    let runtime = origin::runtime_directory(backend, &initialized.selected)?;
    let scope = initialized
        .request
        .pointer("/target/scope_id")
        .and_then(Value::as_str)
        .ok_or("目标请求缺少 scope")?;
    for role in ["api", "worker"] {
        let Some(receipt) = origin::receipt_file(&runtime.join(format!("{role}.json")))? else {
            continue;
        };
        if receipt.get("scope_id").and_then(Value::as_str) != Some(scope)
            || receipt.get("role").and_then(Value::as_str) != Some(role)
        {
            return Err("目标运行收据角色或 scope 变化".into());
        }
        if common::process_identity(
            receipt
                .pointer("/identity/pid")
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or("目标运行收据 PID 无效")?,
        )?
        .is_some()
        {
            return Err("目标业务生产者仍在运行或身份不明".into());
        }
    }
    for key in ["api_url", "worker_ready_url"] {
        let url = initialized
            .selected
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("目标审阅缺少 {key}"))?;
        common::require_closed_port(url)?;
    }
    Ok(())
}
