//! 已发布配额计划与十一租户容量证明的只读入口；不执行 API 或资源写入。
use std::path::Path;

use serde_json::{Value, json};

use super::{history as seed_history, latest};
use crate::{
    Result,
    recovery::clone::native::{digest, json as document, lifecycle::state, resources::common},
};

#[path = "capacity/history.rs"]
mod history;
#[path = "capacity/inputs.rs"]
mod inputs;
#[path = "capacity/model.rs"]
pub(crate) mod model;

use inputs::Reads;

const MODES: &[&str] = &["quotas-plan", "quotas-apply", "quotas-reconcile"];

pub(crate) fn evidence(
    backend: &Path,
    run: &Path,
    request: &Value,
    current: Option<u64>,
) -> Result<Value> {
    common::local_directory(backend, run, "seed 配额运行目录")?;
    let mut reads = Reads::new(backend);
    reads.binding(&run.join("manifest.json"))?;
    reads.binding(&run.join("state.json"))?;
    let saved = state::load(run, true)?;
    let plan = load_plan(&mut reads, run, request, &saved, current)?;
    let record = latest(&saved, "seed-runtime", MODES, current, false)?;
    let result = reads.bound(&record["result"])?;
    let mut confirmed = Vec::new();
    let mut posts = Vec::new();
    for item in document::array(&plan["actions"], "配额动作")? {
        let attempts = directories(&mut reads, run, item)?;
        let attempt = attempts.last().ok_or("配额尚未覆盖全部十一租户")?;
        if let Some(post) = history::confirmation(&mut reads, run, &plan, item, attempt)? {
            posts.push(post);
        }
        confirmed.push(reads.binding(&attempt.join("confirmed.json"))?);
    }
    let binding = reads.binding(&run.join("seed-runtime/quota-plan.json"))?;
    if result["status"] != "seed_quotas_verified"
        || result["plan"] != binding
        || result["confirmed"] != json!(confirmed)
    {
        return Err("配额必须全部确认且最新外层阶段通过后才能创建身份".into());
    }
    for post in posts {
        let after = seed_history::post(backend, run, Some(&post.descriptor), None, false)?;
        if after.request != post.request || after.runtime != post.runtime {
            return Err("配额授权所绑定的 post-copy 在读取期间变化".into());
        }
    }
    reads.check()?;
    if state::load(run, true)? != saved {
        return Err("配额读取期间阶段历史变化；保留现场，禁止自动重放".into());
    }
    reads.check()?;
    Ok(json!({"stage":record["result"],"plan":binding,"confirmed":confirmed}))
}

fn load_plan(
    reads: &mut Reads<'_>,
    run: &Path,
    request: &Value,
    saved: &Value,
    current: Option<u64>,
) -> Result<Value> {
    let record = latest(saved, "seed-runtime", &["quotas-plan"], current, false)?;
    let result = reads.bound(&record["result"])?;
    let path = run.join("seed-runtime/quota-plan.json");
    if result["status"] != "seed_quotas_planned" || result["plan"] != reads.binding(&path)? {
        return Err("配额计划缺少当前登记的外层成功证明".into());
    }
    validate_plan(reads, run, request)
}

fn validate_plan(reads: &mut Reads<'_>, run: &Path, request: &Value) -> Result<Value> {
    let plan = reads.read(&run.join("seed-runtime/quota-plan.json"))?;
    document::exact(
        &plan,
        &[
            "format_version",
            "kind",
            "registration",
            "identity_plan",
            "actions",
            "plan_sha256",
        ],
        "配额计划",
    )?;
    let identity = reads.bound(&request["identity_plan"])?;
    let targets = model::targets(&identity)?;
    let actions = document::array(&plan["actions"], "配额动作")?;
    let mut unhashed = plan.clone();
    unhashed.as_object_mut().unwrap().remove("plan_sha256");
    if plan["format_version"].as_u64() != Some(1)
        || plan["kind"] != "devex-seed-quotas"
        || plan["registration"] != reads.binding(&run.join("seed-runtime.json"))?
        || plan["identity_plan"] != request["identity_plan"]
        || plan["plan_sha256"].as_str() != Some(digest::plan_hash(&unhashed)?.as_str())
        || actions.len() != 11
        || targets.len() != actions.len()
    {
        return Err("配额计划与当前登记、身份计划或摘要不符".into());
    }
    for (target, item) in targets.iter().zip(actions) {
        if *item != model::action(target, &item["observed"])? {
            return Err("配额动作与原完整前像或明确身份计划不同".into());
        }
    }
    Ok(plan)
}

fn directories(reads: &mut Reads<'_>, run: &Path, item: &Value) -> Result<Vec<std::path::PathBuf>> {
    let slot = document::text(&item["target"]["slot"], "配额身份槽")?;
    reads.directories(&run.join("seed-runtime/quotas").join(slot))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/seed/capacity.rs"]
mod tests;
