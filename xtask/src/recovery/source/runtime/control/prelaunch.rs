//! 未启动恢复保留原失败和全部采集历史；只建立一次新的完整观察基线，不启动产品。
use super::{
    configuration,
    frame::Frame,
    observation::{self, Observation, Prepared},
};
use crate::{
    Result,
    recovery::{
        clone::native::{
            digest, export::transport::Transport, lifecycle::state, resources::common,
        },
        source::evidence::running::history::{self, prelaunch},
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(super) struct Plan {
    prepared: Prepared,
    start: Value,
    proof: Value,
    collection: Value,
}

pub(super) fn prepare(
    backend: &Path,
    run: &Path,
    saved: &Value,
    number: u64,
    request: &Path,
) -> Result<Plan> {
    let attempts = state::attempts(saved)?;
    let prefix = if attempts
        .last()
        .is_some_and(|row| row["number"].as_u64() == Some(number) && row["status"] == "running")
    {
        &attempts[..attempts.len() - 1]
    } else {
        attempts
    };
    let (start, collection) = prelaunch::origin(backend, run, prefix)?;
    if prefix
        .last()
        .and_then(|row| row["number"].as_u64())
        .and_then(|n| n.checked_add(1))
        != Some(number)
    {
        return Err("未启动恢复没有绑定原失败和唯一当前采集阶段".into());
    }
    let proof = prelaunch::proof(backend, run, &start)?;
    let descriptor = common::binding(request)?;
    let value = common::read_bound(backend, &descriptor, "未启动恢复正式请求")?;
    let source = history::predecessor(backend, run, saved)?;
    let output = run.join(format!("seed-runtime/attempt-{number:04}"));
    let prepared = observation::prepare(
        backend,
        run,
        number,
        &descriptor,
        &source,
        &output,
        &crate::recovery::source::evidence::generation_request::configuration::directory(
            backend, &value,
        )?,
    )?;
    if prelaunch::proof(backend, run, &start)? != proof || common::binding(request)? != descriptor {
        return Err("未启动恢复准备期间原失败或当前请求变化".into());
    }
    Ok(Plan {
        prepared,
        start,
        proof,
        collection,
    })
}

pub(super) fn run<T: Transport + Sync>(
    frame: &Frame<'_>,
    request: &Path,
    transport: &T,
) -> Result<Value> {
    let plan = prepare(
        frame.backend,
        frame.run,
        frame.state(),
        frame.number,
        request,
    )?;
    let Plan {
        prepared,
        start,
        proof,
        collection,
    } = plan;
    let config = prepared.config.clone();
    frame.checkpoint()?;
    if prelaunch::proof(frame.backend, frame.run, &start)? != proof
        || prelaunch::origin(frame.backend, frame.run, &frame.prefix)?
            != (start.clone(), collection.clone())
    {
        return Err("未启动恢复写入观察目录前原失败或完整历史变化".into());
    }
    let observation = Observation::create(
        frame,
        prepared,
        configuration::Selection::Copy(&config),
        transport,
    )?;
    checkpoint(&observation, &start, &proof, &collection)?;
    let (before, first) = observation.capture("before")?;
    checkpoint(&observation, &start, &proof, &collection)?;
    let (after, last) = observation.capture("after")?;
    if first["image"] != last["image"] {
        return Err("未启动恢复采集期间完整当前像变化，不允许后续启动".into());
    }
    checkpoint(&observation, &start, &proof, &collection)?;
    let request_value = &observation.prepared.facts["request"];
    let mut result = json!({"status":"seed_source_prelaunch_closed","request":observation.prepared.descriptor,
        "failed_start":start["number"],"failure_proof":proof,"collection_failure":collection,
        "history_length":frame.prefix.len(),"history_sha256":digest::plan_hash(&json!(frame.prefix))?,
        "coordinator_source":frame.source(),"runtime":common::binding(&observation.output()?.join("runtime/runtime.json"))?,
        "before":before,"after":after,"historical_image_compared":false,"remote_writes":0,"restore_qualified":false});
    for key in [
        "source_registration",
        "source_rebind",
        "review_successor",
        "current_storage",
    ] {
        result[key] = request_value[key].clone();
    }
    Ok(result)
}

fn checkpoint<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    start: &Value,
    proof: &Value,
    collection: &Value,
) -> Result<()> {
    let frame = observation.frame;
    observation.quiet()?;
    if prelaunch::proof(frame.backend, frame.run, start)? != *proof
        || prelaunch::origin(frame.backend, frame.run, &frame.prefix)?
            != (start.clone(), collection.clone())
    {
        return Err("未启动恢复期间原失败、完整历史或采集失败集合变化".into());
    }
    observation.checkpoint()
}
