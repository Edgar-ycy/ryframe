//! 只读观察原 START；缺少树不等于已停止，不取得控制锁或修补阶段。
use super::existing;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{
            cache,
            lifecycle::{manifest, state},
            resources::common,
            seed::generation::coordinator,
        },
        source::evidence::running::{self, history},
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn run(backend: &Path, run: &Path) -> Result<Value> {
    validate_local_test_path(run, backend, LocalTestPathKind::ExistingDirectory)?;
    let manifest_binding = common::binding(&run.join("manifest.json"))?;
    manifest::load(backend, &run.join("manifest.json"))?;
    let state_binding = common::binding(&run.join("state.json"))?;
    let saved = state::load(run, true)?;
    let attempts = state::attempts(&saved)?;
    let starts = history::starts(backend, run, attempts)?;
    let value = match starts.as_slice() {
        [start] => observe(backend, run, start)?,
        [] => archived(backend, run, attempts)?,
        _ => return Err("source-generation status 需要唯一 START 记录".into()),
    };
    common::bound_file(backend, &manifest_binding, "状态观察原清单")?;
    common::bound_file(backend, &state_binding, "状态观察完整历史")?;
    if state::load(run, true)? != saved {
        return Err("源代次账本在状态观察期间变化；保留现场，不修补或重放".into());
    }
    Ok(value)
}

fn archived(backend: &Path, run: &Path, attempts: &[Value]) -> Result<Value> {
    let replay = history::lineage::authority(backend, run, attempts)?;
    if !replay.is_null() {
        return result(
            &replay["failed"],
            replay["failure"]["proof"]["intent"].clone(),
            roles("replay_authorized"),
        );
    }
    let archive = history::prelaunch::closed(backend, run, attempts)?;
    if !archive.is_null() {
        return result(&archive["start"], Value::Null, roles("not_started"));
    }
    Err("source-generation status 需要唯一 START 或完整已封存恢复记录".into())
}

fn observe(backend: &Path, run: &Path, start: &Value) -> Result<Value> {
    let number = start["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("来源 START 序号无效")?;
    let output = run.join(format!("g{number:04}"));
    if !output.try_exists()? {
        validate_local_test_path(&output, backend, LocalTestPathKind::NewDirectory)?;
        return result(start, Value::Null, roles("unknown"));
    }
    running::directory(backend, &output)?;
    if !output.join("intent.json").try_exists()? {
        unclaimed(&output.join("runtime"))?;
        return result(start, Value::Null, roles("unknown"));
    }
    let request_binding = common::binding(&output.join("request.json"))?;
    let request = common::read_bound(backend, &request_binding, "原 START 请求")?;
    coordinator::request(&request)?;
    let intent_binding = common::binding(&output.join("intent.json"))?;
    let intent = existing::intent(backend, &output, &request_binding)?;
    if start["status"] == "passed" {
        published(backend, run, &output, start, &request)?;
    }
    let source = cache::binding::source::load(backend, &request["review_successor"])?;
    if source["directory"] != json!(run)
        || source["review_successor"]["source_result"] != request["source_registration"]
    {
        return Err("源代次状态请求与原登记、successor 或账本不同".into());
    }
    let observed = existing::observe_roles(
        backend,
        &output.join("runtime"),
        &intent["operations"],
        &source["request"]["source"]["scope_id"],
    )?;
    common::bound_file(backend, &request_binding, "原 START 请求")?;
    common::bound_file(backend, &intent_binding, "原 START 意图")?;
    result(start, intent_binding, observed)
}

fn published(
    backend: &Path,
    run: &Path,
    output: &Path,
    start: &Value,
    request: &Value,
) -> Result<()> {
    let original = common::read_bound(backend, &start["result"], "已发布START结果")?;
    if common::read_bound(backend, &original["request"], "START原显式请求")? != *request {
        return Err("源状态观察的内部请求不同于已发布START".into());
    }
    let saved = state::load(run, true)?;
    let prefix = state::attempts(&saved)?
        .iter()
        .take_while(|row| row["number"] != start["number"])
        .cloned()
        .collect::<Vec<_>>();
    let expected = coordinator::dispatch(&json!({"action":"start-receipt","input":{
        "request":request,"descriptor":original["request"],"coordinator_source":start["sources"],
        "prefix":prefix,"generation_id":output.file_name().and_then(|v|v.to_str()).ok_or("源代次名称无效")?,
        "bindings":{"intent":common::binding(&output.join("intent.json"))?,
        "runtime_evidence":common::binding(&output.join("running-evidence.json"))?,
        "before":common::binding(&output.join("before/image.json"))?,"running":common::binding(&output.join("running/image.json"))?,
        "dataset_lineage":common::binding(&output.join("dataset-lineage.json"))?}}}))?;
    if expected != original {
        return Err("源状态观察的请求、意图或完整证据不属于原发布结果".into());
    }
    common::bound_file(backend, &start["result"], "原已发布START")?;
    common::bound_file(backend, &original["request"], "原显式START请求")?;
    Ok(())
}

fn unclaimed(runtime: &Path) -> Result<()> {
    if !runtime.try_exists()? {
        return Ok(());
    }
    crate::local_test_path::validate_absolute_path(runtime, LocalTestPathKind::ExistingDirectory)?;
    for entry in std::fs::read_dir(runtime)? {
        let entry = entry?;
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if !["binaries.json", "runtime.json"]
            .iter()
            .any(|name| entry.file_name() == *name)
            || !metadata.is_file()
            || crate::recovery::clone::native::digest::linked(&metadata)
            || common::link_count_for(&entry.path(), &metadata)? != 1
        {
            return Err(
                "未发布START意图的runtime存在未知或角色证据；保留现场，不推断未启动".into(),
            );
        }
    }
    Ok(())
}

fn roles(status: &str) -> Value {
    let row = json!({"state":status,"tree":null,"completion":null});
    json!({"api":row,"worker":row})
}

fn result(start: &Value, intent: Value, roles: Value) -> Result<Value> {
    let number = start["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("状态观察 START 序号无效")?;
    Ok(
        json!({"status":"source_generation_observed","attempt":number,"start_status":start["status"],
        "intent":intent,"roles":roles,"remote_writes":0,"restore_qualified":false}),
    )
}
