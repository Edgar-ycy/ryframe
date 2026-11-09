//! 封存前像之后的停机、重启和生成阶段按完整相邻历史核验，不跳过失败证据。
use super::stops;
use crate::{
    Result,
    recovery::clone::native::{json as document, resources::common},
};
use serde_json::{Value, json};
use std::path::Path;
#[path = "segment/failures.rs"]
mod failures;
pub(in crate::recovery::source::evidence) use failures::restart_request;
#[path = "segment/lifecycle.rs"]
mod lifecycle;
#[path = "segment/resources.rs"]
mod resources;

pub(in crate::recovery::source::evidence) fn read(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
    archive: &Value,
    descriptor: &Value,
    current: &Value,
) -> Result<Value> {
    let index = attempts
        .iter()
        .position(|row| *row == archive["recovery"])
        .ok_or("封存恢复不属于完整账本")?;
    let suffix = &attempts[index + 1..];
    if suffix.is_empty() {
        return Ok(Value::Null);
    }
    if operation(&suffix[0]) != ("cache-target", "stop") {
        if suffix
            .iter()
            .any(|row| operation(row) == ("cache-target", "stop"))
        {
            return Err("封存前像后的缓存停机不是首个相邻阶段".into());
        }
        return Ok(Value::Null);
    }
    stop_prefix(archive, suffix)?;
    let cache = stops::cache(
        backend,
        run,
        &suffix[0],
        descriptor,
        &archive["receipt"]["review_successor"],
    )?;
    let storage = stops::storage(backend, run, &suffix[2])?;
    let first = storage["result"]["processes"][0]["attempt"]
        .as_u64()
        .ok_or("存储停止结果缺少首代")?;
    let mut evidence = failures::stop(backend, run, &suffix[1], &suffix[0], first)?;
    if cache["request"] == storage["request"] {
        return Err("缓存与对象存储登记意外共享请求".into());
    }
    let context = Context {
        backend,
        run,
        attempts,
        archive,
        descriptor,
        current,
    };
    let mut resources = resources::read(&context, suffix, &storage["request"])?;
    if let Some(bindings) = resources.get_mut("evidence") {
        evidence.extend(
            document::array(bindings, "分段重启失败证据")?
                .iter()
                .cloned(),
        );
    }
    let count = resources["count"].as_u64().ok_or("分段资源阶段数量无效")? as usize;
    let tail = &suffix[3 + count..];
    if !tail.is_empty() && resources["phase"] != "ready" {
        return Err("分段资源恢复完成前不能执行生成、发布或其他控制动作".into());
    }
    let lifecycle = lifecycle::read(&context, tail, suffix, &cache, &storage, &resources)?;
    let mut records = suffix[..3 + count].to_vec();
    records.extend(
        document::array(&lifecycle["records"], "分段生命周期已核验阶段")?
            .iter()
            .cloned(),
    );
    for binding in &evidence {
        common::bound_file(backend, binding, "分段失败或控制器证据")?;
    }
    Ok(
        json!({"phase":lifecycle["phase"],"records":records,"storage":lifecycle["storage"],
        "cache":lifecycle["cache"],"replay":lifecycle["replay"],"reboot":lifecycle["reboot"]}),
    )
}

fn operation(row: &Value) -> (&str, &str) {
    (
        row["stage"].as_str().unwrap_or(""),
        row["mode"].as_str().unwrap_or(""),
    )
}

fn number(row: &Value) -> Result<u64> {
    row["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "分段阶段序号无效".into())
}

fn product(reference: &Value, row: &Value) -> Result<()> {
    crate::recovery::source::evidence::staging::validate_source(reference)?;
    crate::recovery::source::evidence::staging::validate_source(&row["sources"])?;
    if row["sources"]["fingerprints"]["product"] != reference["fingerprints"]["product"] {
        return Err("分段续作改变了 seed 产品来源".into());
    }
    if row["sources"]["snapshot"]["files"] != json!([])
        || row["sources"]["snapshot"]["patch_sha256"] != crate::contract::sha256_hex(b"")
    {
        return Err("分段资源停机必须来自完整干净源码".into());
    }
    Ok(())
}

fn owner(run: &Path, attempts: &[Value], row: &Value, current: &Value) -> Result<()> {
    if row["number"] != *current
        || attempts.last() != Some(row)
        || row["status"] != "running"
        || !row["result"].is_null()
        || !row["error_type"].is_null()
    {
        return Err("分段续作控制动作不是当前唯一持锁阶段".into());
    }
    super::resolution::owner(run)
}

struct Context<'a> {
    backend: &'a Path,
    run: &'a Path,
    attempts: &'a [Value],
    archive: &'a Value,
    descriptor: &'a Value,
    current: &'a Value,
}

fn stop_prefix(archive: &Value, suffix: &[Value]) -> Result<()> {
    let expected = [
        ("cache-target", "stop", "passed"),
        ("storage-target", "stop", "failed"),
        ("storage-target", "stop", "passed"),
    ];
    let base = number(&archive["recovery"])?;
    if suffix.len() < 3 {
        return Err("封存前像后的分段停机历史不完整".into());
    }
    for (offset, (row, (stage, mode, status))) in suffix.iter().zip(expected).enumerate() {
        if number(row)?
            != base
                .checked_add(u64::try_from(offset + 1)?)
                .ok_or("分段序号溢出")?
            || row["stage"] != stage
            || row["mode"] != mode
            || row["status"] != status
        {
            return Err("封存前像后的分段停机历史不完整或不相邻".into());
        }
    }
    let reference = &archive["recovery"]["sources"];
    if suffix[0]["sources"] != *reference || suffix[1]["sources"] != *reference {
        return Err("缓存停止和首次存储停止必须使用封存时的完整工具来源".into());
    }
    for row in &suffix[..3] {
        product(reference, row)?;
    }
    Ok(())
}
