//! 已发布代次的只读组合；复用 START、来源验收、完整像和 generation 模型。
use super::super::{execution, images};
use super::{common, document, path};
use crate::{
    Result,
    recovery::clone::native::{export::input, seed::generation::coordinator},
};
use serde_json::{Value, json};
use std::path::Path;

mod quiescence;

pub(super) fn model(action: &str, input: Value) -> Result<Value> {
    let value = coordinator::dispatch(&json!({"action":action,"input":input}))?;
    if value.get("rejected").is_some() {
        return Err("已发布来源代次模型拒绝绑定或完整历史".into());
    }
    Ok(value)
}

pub(super) fn resolve(backend: &Path, source: &Value, state: &Value) -> Result<Value> {
    let selected = model("select-published", json!({"attempts":state["attempts"]}))?;
    let record = &selected["record"];
    let mut result = source.clone();
    result["source_request"] = source["registration"]["source_request"].clone();
    result["source_environment"] = source["registration"]["source_environment"].clone();
    result["generation_verified"] = source["registration"]["generation_verified"].clone();
    result["source_generation"] = Value::Null;
    if record.is_null() {
        return Ok(result);
    }
    let run = path(&source["directory"])?;
    let number = record["number"].as_u64().ok_or("STOP 序号无效")?;
    if common::bound_file(backend, &record["result"], "STOP 发布")?
        != run.join(format!("results/{number:04}.json"))
    {
        return Err("STOP 发布不属于同一固定 attempt".into());
    }
    let value = common::read_bound(backend, &record["result"], "STOP 发布")?;
    bindings(backend, &value)?;
    let verified = execution::saved::verify(backend, &value["source_runtime"], false)?;
    let facts = &verified["facts"];
    let request = &facts["request"];
    let descriptor = &source["review_successor"]["source_result"];
    model(
        "published",
        json!({"receipt":value,"request":request,"source":source,
        "descriptor":descriptor,"prefix":selected["prefix"],"source_receipt":facts["receipt"],
        "verified_source":facts["source"],"verified_receipt":verified["receipt"],
        "before":facts["receipt"]["before"],"running":facts["receipt"]["running"]}),
    )?;
    let output = path(&facts["output"])?;
    same_generation_files(backend, &output, request, &value)?;
    let (effective, build) = effective_request(backend, &output, source, request, &value)?;
    let before = images::verify(
        backend,
        &value["stop_before"],
        &effective["source"],
        &source["request"],
        descriptor,
    )?;
    let after = images::verify(
        backend,
        &value["after"],
        &effective["source"],
        &source["request"],
        descriptor,
    )?;
    if before["image"] != after["image"] || before["image"] != verified["after"]["image"] {
        return Err("来源验收后或停止期间完整像发生未知变化".into());
    }
    let runtime = quiescence::verify(backend, &output, request, &effective)?;
    let generation = common::read_bound(backend, &value["generation_verified"], "后继 generation")?;
    let execution = path(&facts["execution"])?;
    model(
        "generation-proof",
        json!({"effective":effective,
        "runtime":common::read_bound(backend,&effective["runtime"],"后继 runtime")?,
        "source_snapshot":build["sources"]["full"]["source"]["snapshot"],
        "maintenance":common::read_bound(&execution,&effective["maintenance_build"],"后继维护构建")?,
        "processes":{"api":common::read_bound(backend,&effective["processes"]["api"],"API 身份")?["identity"],
                     "worker":common::read_bound(backend,&effective["processes"]["worker"],"Worker 身份")?["identity"]},
        "producers":common::read_bound(backend,&effective["producers_registry"],"后继生产者")?["processes"],
        "physical_binding":source["generation"]["physical_binding"],"generation":generation,
        "runtime_physical":runtime["physical_binding"]}),
    )?;
    result["request"] = effective;
    result["source_request"] = value["source_request"].clone();
    result["generation"] = generation;
    result["generation_verified"] = value["generation_verified"].clone();
    result["source_generation"] = record["result"].clone();
    result["source_environment"] = request["source_environment"].clone();
    result["environment"] =
        common::read_bound(backend, &request["source_environment"], "后继私有环境")?;
    bindings(backend, &value)?;
    Ok(result)
}

pub(super) fn bindings(backend: &Path, value: &Value) -> Result<()> {
    match value {
        Value::Object(object)
            if object.len() == 3
                && ["path", "bytes", "sha256"]
                    .iter()
                    .all(|key| object.contains_key(*key)) =>
        {
            common::bound_file(backend, value, "比较来源完整证据绑定")?;
        }
        Value::Object(object) => {
            for value in object.values() {
                bindings(backend, value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                bindings(backend, value)?;
            }
        }
        _ => (),
    }
    Ok(())
}

fn same_generation_files(
    backend: &Path,
    output: &Path,
    request: &Value,
    value: &Value,
) -> Result<()> {
    for (key, name) in [
        ("source_request", "source-request.json"),
        ("generation_verified", "generation-verified.json"),
        ("runtime_evidence", "runtime-evidence.json"),
        ("before", "before/image.json"),
        ("running", "running/image.json"),
        ("stop_before", "stop-before/image.json"),
        ("after", "after/image.json"),
    ] {
        if common::bound_file(backend, &value[key], "STOP 同代证据")? != output.join(name) {
            return Err("STOP 证据不属于同一固定代次目录".into());
        }
    }
    if document::read(&output.join("request.json"))? != *request {
        return Err("STOP 请求副本与原绑定不同".into());
    }
    Ok(())
}

fn effective_request(
    backend: &Path,
    output: &Path,
    source: &Value,
    request: &Value,
    value: &Value,
) -> Result<(Value, Value)> {
    let effective = common::read_bound(backend, &value["source_request"], "后继源请求")?;
    input::validate_request(backend, &effective)?;
    let runtime_dir = output.join("runtime");
    let build = common::read_bound(backend, &request["backend_build"], "后继源构建")?;
    let expected = model(
        "source-request",
        json!({"source_request":source["request"],"request":request,
        "fingerprint":build["sources"]["full"]["source"]["worktree_fingerprint"],
        "runtime":common::binding(&runtime_dir.join("runtime.json"))?,
        "processes":{"api":common::binding(&runtime_dir.join("api.json"))?,"worker":common::binding(&runtime_dir.join("worker.json"))?},
        "producers_registry":common::binding(&output.join("producers.json"))?,"runtime_dir":runtime_dir}),
    )?;
    if effective != expected {
        return Err("后继请求改变了同一物理来源或构建绑定".into());
    }
    Ok((effective, build))
}
