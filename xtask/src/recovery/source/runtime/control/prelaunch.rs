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
        source::evidence::running::history::{
            self,
            prelaunch::{self, revalidated},
        },
    },
};
use serde_json::{Value, json};
use std::path::Path;

#[path = "prelaunch/revalidated.rs"]
mod complete_image;
pub(super) use complete_image::{preflight as preflight_revalidated, run as run_revalidated};

pub(super) struct Plan {
    prepared: Prepared,
    evidence: FailureEvidence,
}

#[derive(PartialEq)]
struct FailureEvidence {
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
    let prepared = prepare_observation(backend, run, saved, number, &descriptor)?;
    if prelaunch::proof(backend, run, &start)? != proof || common::binding(request)? != descriptor {
        return Err("未启动恢复准备期间原失败或当前请求变化".into());
    }
    Ok(Plan {
        prepared,
        evidence: FailureEvidence {
            start,
            proof,
            collection,
        },
    })
}

fn prepare_observation(
    backend: &Path,
    run: &Path,
    saved: &Value,
    number: u64,
    descriptor: &Value,
) -> Result<Prepared> {
    let value = common::read_bound(backend, descriptor, "未启动恢复正式请求")?;
    let source = history::predecessor(backend, run, saved)?;
    let output = run.join(format!("seed-runtime/attempt-{number:04}"));
    observation::prepare(
        backend,
        run,
        number,
        descriptor,
        &source,
        &output,
        &crate::recovery::source::evidence::generation_request::configuration::directory(
            backend, &value,
        )?,
    )
}

pub(super) fn preflight_partial(
    backend: &Path,
    run: &Path,
    prefix: &[Value],
    number: u64,
    request: &Path,
    start: &Value,
) -> Result<()> {
    let descriptor = common::binding(request)?;
    partial_evidence(backend, run, prefix, number, start, &descriptor)?;
    Ok(())
}

pub(super) fn run_partial<T: Transport + Sync>(
    frame: &Frame<'_>,
    request: &Path,
    start: &Value,
    transport: &T,
) -> Result<Value> {
    let descriptor = common::binding(request)?;
    let evidence = partial_evidence(
        frame.backend,
        frame.run,
        &frame.prefix,
        frame.number,
        start,
        &descriptor,
    )?;
    // 来源前序核验要求同一账本已有当前持锁恢复记录。
    let prepared = prepare_observation(
        frame.backend,
        frame.run,
        frame.state(),
        frame.number,
        &descriptor,
    )?;
    observe(frame, Plan { prepared, evidence }, transport)
}

fn partial_evidence(
    backend: &Path,
    run: &Path,
    prefix: &[Value],
    number: u64,
    start: &Value,
    descriptor: &Value,
) -> Result<FailureEvidence> {
    let (original, proof, collection) =
        prelaunch::partial::recovery_history(backend, run, prefix, number, descriptor)?;
    if original != *start {
        return Err("部分启动前恢复没有绑定同一原失败代次".into());
    }
    Ok(FailureEvidence {
        start: original,
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
    observe(frame, plan, transport)
}

fn observe<T: Transport + Sync>(frame: &Frame<'_>, plan: Plan, transport: &T) -> Result<Value> {
    let Plan { prepared, evidence } = plan;
    let config = prepared.config.clone();
    frame.checkpoint()?;
    if !evidence.unchanged(frame, &prepared.descriptor)? {
        return Err("未启动恢复写入观察目录前原失败或完整历史变化".into());
    }
    let observation = Observation::create(
        frame,
        prepared,
        configuration::Selection::Copy(&config),
        transport,
    )?;
    checkpoint(&observation, &evidence)?;
    let (before, first) = observation.capture("before")?;
    checkpoint(&observation, &evidence)?;
    let (after, last) = observation.capture("after")?;
    if first["image"] != last["image"] {
        return Err("未启动恢复采集期间完整当前像变化，不允许后续启动".into());
    }
    checkpoint(&observation, &evidence)?;
    let status = if prelaunch::partial::applies(&evidence.start) {
        "seed_source_partial_prelaunch_closed"
    } else {
        "seed_source_prelaunch_closed"
    };
    receipt(&observation, status, &evidence, &before, &after, false)
}

fn receipt<T: Transport + Sync>(
    observation: &Observation<'_, '_, T>,
    status: &str,
    evidence: &FailureEvidence,
    before: &Value,
    after: &Value,
    historical_image_compared: bool,
) -> Result<Value> {
    let frame = observation.frame;
    let request_value = &observation.prepared.facts["request"];
    let mut result = json!({"status":status,"request":observation.prepared.descriptor,
        "failed_start":evidence.start["number"],"failure_proof":evidence.proof,"collection_failure":evidence.collection,
        "history_length":frame.prefix.len(),"history_sha256":digest::plan_hash(&json!(frame.prefix))?,
        "coordinator_source":frame.source(),"runtime":common::binding(&observation.output()?.join("runtime/runtime.json"))?,
        "before":before,"after":after,"historical_image_compared":historical_image_compared,"remote_writes":0,"restore_qualified":false});
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
    evidence: &FailureEvidence,
) -> Result<()> {
    observation.quiet()?;
    if !evidence.unchanged(observation.frame, &observation.prepared.descriptor)? {
        return Err("未启动恢复期间原失败、完整历史或采集失败集合变化".into());
    }
    observation.checkpoint()
}

impl FailureEvidence {
    fn unchanged(&self, frame: &Frame<'_>, descriptor: &Value) -> Result<bool> {
        if prelaunch::partial::applies(&self.start) {
            return Ok(partial_evidence(
                frame.backend,
                frame.run,
                &frame.prefix,
                frame.number,
                &self.start,
                descriptor,
            )? == *self);
        }
        Ok(
            prelaunch::proof(frame.backend, frame.run, &self.start)? == self.proof
                && prelaunch::origin(frame.backend, frame.run, &frame.prefix)?
                    == (self.start.clone(), self.collection.clone()),
        )
    }
}
