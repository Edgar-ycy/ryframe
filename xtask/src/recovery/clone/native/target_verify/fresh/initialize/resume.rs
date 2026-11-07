//! 显式恢复先重验完整前像；库存恢复不写资源，迁移恢复不重放成功操作。
mod attempt;
mod migration;
use super::super::super::super::{
    export::transport::Transport, json as document, resources::common,
};
use super::super::super::{reset, resume as model, resume_runtime, resume_state};
use super::super::{failure, receipts, registration::Registration};
use super::{claims, operations, window::Window};
use crate::Result;
pub(crate) use attempt::Attempt;
use serde_json::{Value, json};

pub(crate) fn state(reg: &Registration, proof: &Value, owned: bool) -> Result<Value> {
    let mut inventory = model::inventory::state(
        &reg.backend,
        &reg.target,
        &reg.value["request"],
        proof,
        owned,
    )?;
    if inventory["resumable"] == true {
        inventory["mode"] = json!("inventory");
        return Ok(inventory);
    }
    let migration = resume_state::state(
        &reg.backend,
        &reg.target,
        &reg.value["request"],
        proof,
        owned,
    )?;
    if migration["resumable"] != true {
        return Err("初始化现场不能明确续作；核对库存失败或连续迁移前缀，禁止重放".into());
    }
    if !migration["predecessor"].is_null() {
        let prior = common::read_bound(&reg.backend, &migration["predecessor"], "续作前驱")?;
        let started = common::read_bound(&reg.backend, &prior["started"], "续作前驱实际代次")?;
        resume_runtime::started(&reg.backend, &migration["prepared"]["generation"], &started)?;
        if resume_state::state(
            &reg.backend,
            &reg.target,
            &reg.value["request"],
            proof,
            owned,
        )? != migration
        {
            return Err("运行代次核验期间完整续作树变化".into());
        }
    }
    Ok(migration)
}
pub(crate) fn execute<T: Transport>(
    w: &Window<'_, T>,
    proof: &Value,
    attempt: &mut Attempt,
) -> Result<(Value, Value)> {
    if state(w.registration, proof, true)? != attempt.state {
        return Err("取得初始化锁前续作前像发生变化".into());
    }
    if attempt.state["mode"] == "inventory" {
        inventory(w, attempt)
    } else {
        migration::execute(w, attempt)
    }
}
fn inventory<T: Transport>(w: &Window<'_, T>, attempt: &mut Attempt) -> Result<(Value, Value)> {
    let target = w.inputs.observation_dir;
    let intent=attempt.transition(w,"inventory-intent",&json!({"attempt":attempt.id,"at":attempt.now(),"failure":attempt.state["failure"],"prepared_files":attempt.state["prepared_files"]}))?;
    attempt.publish_intent(&intent)?;
    let mut trusted = claims::one(w, &attempt.state["files"], &attempt.intent, Some(&intent))?;
    let prepared = document::read(&target.join("prepare.json"))?;
    claims::unchanged(w, &trusted)?;
    let (current, candidate, _) = w.generation()?;
    trusted = claims::runtime(w, &trusted, &candidate)?;
    if current != prepared["generation"] {
        return Err("库存恢复前来源、工具或存储实际代次变化".into());
    }
    claims::unchanged(w, &trusted)?;
    let label = format!("resume-reset-plan-{}", attempt.id);
    let plan = operations::reset_plan(w, &current, &label)?;
    let command = attempt.command_paths(w, &label, &trusted)?;
    if command.len() != 1 {
        return Err("库存恢复没有唯一reset plan命令收据".into());
    }
    let plan_path = target.join(format!("{label}.json"));
    trusted = claims::added(
        w,
        &trusted,
        &std::collections::BTreeSet::from([
            plan_path.file_name().unwrap().to_str().unwrap().to_owned(),
            command[0].file_name().unwrap().to_str().unwrap().to_owned(),
        ]),
    )?;
    let snapshot = super::reset_configuration::Snapshot::open(
        w.inputs.backend,
        target,
        w.inputs.request,
        &current["maintenance"],
    )?;
    let reset_request = snapshot.request(w.inputs.request)?;
    reset::plan_receipt(
        w.inputs.backend,
        target,
        &command[0],
        w.inputs
            .maintenance_executable("reset")?
            .to_str()
            .ok_or("reset路径不是UTF8")?,
        &plan,
        &reset_request,
    )?;
    let mut done = reset::completed(
        w.inputs.backend,
        target,
        &plan["manifest"],
        document::text(&plan["plan_hash"], "reset hash")?,
    )?;
    done["configuration"] = snapshot.binding()?;
    if document::read(&target.join("reset.confirmed.json"))?["observed"] != done {
        return Err("库存恢复reset完成前像不同于当前计划".into());
    }
    complete(w, attempt, trusted, &current, &done, None)
}
pub(super) fn complete<T: Transport>(
    w: &Window<'_, T>,
    attempt: &mut Attempt,
    mut trusted: Value,
    current: &Value,
    reset: &Value,
    marker: Option<Value>,
) -> Result<(Value, Value)> {
    let target = w.inputs.observation_dir;
    let name = format!("inventory-resume-{}", attempt.id);
    claims::unchanged(w, &trusted)?;
    let captured = failure::capture(super::super::super::database::capture_at(
        &w.inputs,
        &target.join(&name),
        w.transport,
        &|| w.checkpoint(),
    ))?;
    let initial = json!({"receipt":captured.receipt,"observations":captured.observations});
    let paths = claims::inventory(w, &name, &initial, &captured.evidence_files)?;
    trusted = claims::added(w, &trusted, &paths)?;
    claims::unchanged(w, &trusted)?;
    let objects = w.objects(true)?;
    trusted = claims::objects(w, &trusted, &objects)?;
    let redis = w.redis_state(current, true, true)?;
    claims::unchanged(w, &trusted)?;
    let (observed, candidate, _) = w.generation()?;
    if observed != *current {
        return Err("续作最终发布前实际generation变化".into());
    }
    trusted = claims::runtime(w, &trusted, &candidate)?;
    common::bound_file(
        w.inputs.backend,
        &w.registration.value["request"],
        "固定请求",
    )?;
    let expected = if attempt.state["mode"] == "inventory" {
        current.clone()
    } else {
        attempt.state["prepared"]["generation"].clone()
    };
    let value=attempt.transition(w,"initialized",&json!({"request":w.inputs.request,"generation":expected,"reset":reset,"inventory":initial,"objects":objects,"redis":redis,"at":attempt.now()}))?;
    for name in ["initialized-candidate.json", "initialized.json"] {
        claims::unchanged(w, &trusted)?;
        let path = target.join(name);
        receipts::write(&path, &value)?;
        trusted = claims::one(w, &trusted, &path, Some(&value))?;
    }
    let args = attempt.confirmation(w, marker)?;
    let kind = if attempt.state["mode"] == "inventory" {
        "inventory-confirmed"
    } else {
        "migration-confirmed"
    };
    let confirmation = attempt.transition(w, kind, &args)?;
    let path = target.join(format!("resume-initialize-{}.confirmed.json", attempt.id));
    receipts::write(&path, &confirmation)?;
    trusted = claims::one(w, &trusted, &path, Some(&confirmation))?;
    claims::unchanged(w, &trusted)?;
    Ok((value, trusted))
}
