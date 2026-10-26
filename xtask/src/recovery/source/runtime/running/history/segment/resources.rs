//! 分段资源恢复只采用完整已登记代次；运行中的资源动作必须仍由当前锁持有者控制。
use super::super::resolution::cache_call;
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{cache, resources::storage::registered, seed::protocol::guards},
};
use serde_json::{Value, json};

pub(super) fn read(
    context: &super::Context<'_>,
    suffix: &[Value],
    storage_request: &Value,
) -> Result<Value> {
    let rest = &suffix[3..];
    let mut phase = "stopped";
    let mut storage = Value::Null;
    let mut cache = Value::Null;
    let mut evidence = Vec::new();
    let mut offset = 0;
    if rest.first().is_some_and(|row| {
        super::operation(row) == ("storage-target", "restart") && row["status"] == "failed"
    }) {
        let failed = &rest[0];
        super::product(&context.archive["recovery"]["sources"], failed)?;
        adjacent(&suffix[2], failed)?;
        evidence = super::failures::restart(
            context.backend,
            context.run,
            failed,
            storage_request,
            rest.len() == 1 && context.current.is_null(),
        )?;
        phase = "storage-retryable";
        offset = 1;
    }
    if let Some(restart) = rest.get(offset) {
        let previous = if offset == 0 { &suffix[2] } else { &rest[0] };
        adjacent(previous, restart)?;
        (phase, storage) = storage_restart(context, restart, rest.len() == offset + 1)?;
    }
    let cache_index = offset + 1;
    if let Some(restart) = rest.get(cache_index) {
        adjacent(&rest[cache_index - 1], restart)?;
        if phase != "storage-ready" {
            return Err("对象存储后必须执行唯一缓存重启".into());
        }
        (phase, cache) = cache_restart(context, restart, rest.len() == cache_index + 1)?;
    }
    Ok(
        json!({"phase":phase,"storage":storage,"cache":cache,"evidence":evidence,
        "count":rest.len().min(cache_index+1)}),
    )
}

fn storage_restart(
    context: &super::Context<'_>,
    restart: &Value,
    last: bool,
) -> Result<(&'static str, Value)> {
    let backend = context.backend;
    let run = context.run;
    super::product(&context.archive["recovery"]["sources"], restart)?;
    if super::operation(restart) != ("storage-target", "restart") {
        return Err("分段停机后必须先执行唯一对象存储重启".into());
    }
    if restart["status"] == "running" {
        super::owner(run, context.attempts, restart, context.current)?;
        if !last {
            return Err("未完成对象存储重启后出现其他阶段".into());
        }
        return Ok(("storage-running", Value::Null));
    }
    if restart["status"] != "passed" || !restart["error_type"].is_null() {
        return Err("分段对象存储重启失败或未收尾".into());
    }
    let before = super::number(restart)?
        .checked_add(1)
        .ok_or("存储重启序号溢出")?;
    let storage = registered::registered(backend, run, CloneSide::Target, Some(before))?
        .unwrap_or(Value::Null);
    if storage["attempt"] != restart["number"] || storage["restart_result"] != restart["result"] {
        return Err("分段对象存储重启没有绑定同一成功阶段".into());
    }
    guards::rebind::live::transition(
        backend,
        &json!({"original":context.archive["receipt"]["current_storage"],"current":storage}),
    )?;
    Ok(("storage-ready", storage))
}

fn cache_restart(
    context: &super::Context<'_>,
    restart: &Value,
    last: bool,
) -> Result<(&'static str, Value)> {
    let backend = context.backend;
    let run = context.run;
    super::product(&context.archive["recovery"]["sources"], restart)?;
    if super::operation(restart) != ("cache-target", "restart") {
        return Err("对象存储后必须执行唯一缓存重启".into());
    }
    if !matches!(restart["status"].as_str(), Some("running" | "passed"))
        || restart["status"] == "passed" && !restart["error_type"].is_null()
    {
        return Err("分段缓存重启失败或未收尾".into());
    }
    let frozen = if restart["status"] == "passed" {
        context.archive["receipt"]["review_successor"].clone()
    } else {
        Value::Null
    };
    let successor = cache_call(
        backend,
        run,
        "history-proof",
        json!({"attempt":restart,"source_result":context.descriptor,"frozen_successor":frozen}),
    )?;
    if successor != context.archive["receipt"]["review_successor"] {
        return Err("缓存重启没有继承封存请求的同一 successor".into());
    }
    if restart["status"] == "running" {
        super::owner(run, context.attempts, restart, context.current)?;
        if !last {
            return Err("未完成缓存重启后出现其他阶段".into());
        }
        return Ok(("cache-running", Value::Null));
    }
    let before = super::number(restart)?
        .checked_add(1)
        .ok_or("缓存重启序号溢出")?;
    let cache = cache::binding::query(
        backend,
        run,
        None,
        &json!(before),
        &json!([
            context.descriptor,
            context.archive["receipt"]["review_successor"]
        ]),
    )?;
    let registration = cache_call(backend, run, "registration", json!({}))?;
    if cache["attempt"] != restart["number"]
        || cache["restart_result"] != restart["result"]
        || cache["request"] != registration[1]
    {
        return Err("分段缓存重启没有绑定同一成功阶段和固定请求".into());
    }
    Ok(("ready", cache))
}

fn adjacent(previous: &Value, next: &Value) -> Result<()> {
    if super::number(previous)?.checked_add(1) != Some(super::number(next)?) {
        return Err("分段资源续作阶段不相邻".into());
    }
    Ok(())
}
