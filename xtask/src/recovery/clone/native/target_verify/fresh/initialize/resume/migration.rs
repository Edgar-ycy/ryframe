//! 连续迁移前缀后仅显式执行缺失操作；每个写操作独立intent与确认。
use super::super::super::super::super::{
    export::transport::Transport, json as document, resources::common,
};
use super::super::super::super::{
    binding_request, resume_prefix, resume_runtime, storage::generation,
};
use super::super::super::receipts;
use super::super::{claims, marker, operations, window::Window};
use super::{Attempt, complete};
use crate::Result;
use serde_json::{Value, json};

pub(super) fn execute<T: Transport>(w: &Window<'_, T>, a: &mut Attempt) -> Result<(Value, Value)> {
    let target = w.inputs.observation_dir;
    let successor = a.state["mode"] == "migration-successor";
    let (mut trusted, current, restart, before) = begin(w, a, successor)?;
    let mut marker = if successor {
        Some(a.state["prior_marker_confirmation"].clone())
    } else {
        None
    };
    if !restart.is_null() && !successor {
        a.session = a.transition(w, "session-markers", &json!({"session":a.session}))?;
        marker = Some(marker::execute(
            w,
            marker::Input {
                attempt: &a.id,
                resume: &a.intent,
                started: &a.started,
                generation: &restart,
                before: &before["redis"],
            },
            &mut a.session["active_operation"],
            &mut trusted,
        )?);
        a.session = a.transition(
            w,
            "session-markers-completed",
            &json!({"session":a.session,"confirmation":marker}),
        )?;
        refresh(w, &mut trusted, &current)?;
    }
    remaining(w, a, &mut trusted, &current)?;
    resume_prefix::prefix(w.inputs.backend, target, &a.state["operations"], true)?;
    let reset = a.state["reset"]["completed"].clone();
    complete(w, a, trusted, &current, &reset, marker)
}
fn begin<T: Transport>(
    w: &Window<'_, T>,
    a: &mut Attempt,
    successor: bool,
) -> Result<(Value, Value, Value, Value)> {
    let binaries =
        binding_request::binaries(w.inputs.backend, w.inputs.request)?["bindings"].clone();
    if successor && binaries != a.state["protected_binaries"] {
        return Err("后继受保护二进制不同于原续作".into());
    }
    let intent=a.transition(w,"migration-intent",&json!({"attempt":a.id,"at":a.now(),"request":w.registration.value["request"],"state":a.state,"protected_binaries":binaries}))?;
    a.publish_intent(w, &intent)?;
    let mut trusted = claims::one(w, &a.state["files"], &a.intent, Some(&intent))?;
    claims::unchanged(w, &trusted)?;
    let (current, candidate, cache) = w.generation()?;
    trusted = claims::runtime(w, &trusted, &candidate)?;
    let original = &a.state["prepared"]["generation"];
    generation::validate_transition(
        &original["storage"],
        &current["storage"],
        candidate.runtime.as_ref(),
        cache.as_ref(),
    )?;
    let mut allowed = original.clone();
    allowed["storage"] = current["storage"].clone();
    if allowed != current {
        return Err("迁移续作配置、源码或非存储代次变化".into());
    }
    let storage = candidate.runtime.clone().unwrap_or(Value::Null);
    let cache = cache.unwrap_or(Value::Null);
    let restart = resume_runtime::fixture(&storage, &cache)?;
    if successor && restart != a.state["restart_generation"] {
        return Err("迁移后继使用了不同夹具服务代次".into());
    }
    claims::unchanged(w, &trusted)?;
    let before = preimage(w, successor || restart.is_null())?;
    if successor && before != a.state["expected_resources"] {
        return Err("迁移后继资源前像不同于首次已确认状态".into());
    }
    let started=a.transition(w,"migration-started",&json!({"attempt":a.id,"at":a.now(),"intent":common::binding(&a.intent)?,"generation":current,
        "storage_runtime":storage,"cache_runtime":cache,"resources_before":before}))?;
    receipts::write(&a.started, &started)?;
    trusted = claims::preimage(w, &trusted, &before["objects"], &a.started, &started)?;
    Ok((trusted, current, restart, before))
}
fn remaining<T: Transport>(
    w: &Window<'_, T>,
    a: &mut Attempt,
    trusted: &mut Value,
    current: &Value,
) -> Result<()> {
    let next = a.state["next_index"]
        .as_u64()
        .and_then(|v| usize::try_from(v).ok())
        .ok_or("迁移序号无效")?;
    let sequence = document::array(&a.state["operations"], "十迁移")?.to_vec();
    for (index, operation) in sequence.iter().enumerate().skip(next) {
        a.session = a.transition(
            w,
            "session-operation",
            &json!({"session":a.session,"operation":operation,"index":index}),
        )?;
        let completed = run_operation(w, a, operation, index, trusted)?;
        a.session = a.transition(
            w,
            "session-completed",
            &json!({"session":a.session,"operation":operation,"completed":completed}),
        )?;
        refresh(w, trusted, current)?;
    }
    Ok(())
}
fn preimage<T: Transport>(w: &Window<'_, T>, markers: bool) -> Result<Value> {
    let databases = document::array(&w.inputs.request["target"]["databases"], "四目标")?
        .iter()
        .map(|db| operations::database(w, db, false))
        .collect::<Result<Vec<_>>>()?;
    Ok(
        json!({"databases":databases,"objects":w.objects(true)?,"redis":w.redis_state(markers,markers)?}),
    )
}
fn refresh<T: Transport>(w: &Window<'_, T>, trusted: &mut Value, current: &Value) -> Result<()> {
    claims::unchanged(w, trusted)?;
    let (observed, candidate, _) = w.generation()?;
    if observed != *current {
        return Err("迁移续作期间实际generation变化".into());
    }
    *trusted = claims::runtime(w, trusted, &candidate)?;
    Ok(())
}
fn run_operation<T: Transport>(
    w: &Window<'_, T>,
    a: &mut Attempt,
    operation: &Value,
    index: usize,
    trusted: &mut Value,
) -> Result<Value> {
    let target = w.inputs.observation_dir;
    let path = target.join(format!("resume-migrate-{}-{index:02}.intent.json", a.id));
    a.session["active_operation"] = json!({"id":operation["id"],"index":index,"intent":null,"command_receipts":[],"phase":"operation-intent-checkpoint"});
    let value=a.transition(w,"operation-intent",&json!({"attempt":a.id,"index":index,"at":a.now(),"resume":common::binding(&a.intent)?,"failure":a.state["failure"],"operation":operation}))?;
    claims::unchanged(w, trusted)?;
    receipts::write(&path, &value)?;
    *trusted = claims::one(w, trusted, &path, Some(&value))?;
    a.session["active_operation"]["intent"] = common::binding(&path)?;
    a.session["active_operation"]["phase"] = json!("pre-command-tree-check");
    claims::unchanged(w, trusted)?;
    let stage = document::text(&operation["stage"], "迁移阶段")?;
    a.session["active_operation"]["phase"] = json!("command-running");
    let result = w.command(
        stage,
        document::array(&operation["command"], "固定argv")?,
        1800,
    );
    let paths = a.command_paths(w, stage, trusted)?;
    a.session["active_operation"]["command_receipts"] = json!(
        paths
            .iter()
            .map(|p| common::binding(p))
            .collect::<Result<Vec<_>>>()?
    );
    result?;
    a.session["active_operation"]["phase"] = json!("command-receipt-verify");
    let receipt = resume_prefix::operation_record(w.inputs.backend, target, operation)?;
    *trusted = claims::one(
        w,
        trusted,
        common::bound_file(w.inputs.backend, &receipt, "迁移命令收据")?.as_path(),
        None,
    )?;
    let confirmation=a.transition(w,"operation-confirmed",&json!({"attempt":a.id,"index":index,"at":a.now(),"intent":common::binding(&path)?,"receipt":receipt,"write":operation["write"]}))?;
    a.session["active_operation"]["phase"] = json!("operation-confirmation-publish");
    let path = target.join(format!("resume-migrate-{}-{index:02}.confirmed.json", a.id));
    receipts::publish(w.transport, &path, &confirmation)?;
    *trusted = claims::one(w, trusted, &path, Some(&confirmation))?;
    Ok(json!({"id":operation["id"],"receipt":receipt,"confirmation":common::binding(&path)?}))
}
