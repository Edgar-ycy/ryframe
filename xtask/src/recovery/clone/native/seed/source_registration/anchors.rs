//! 来源登记锚点的内部组合；控制器授权和真实 quiet 观察由父流程提供。
use super::{close, copied, identity, lineage, materialize::Session, storage};
use crate::{
    Result,
    recovery::clone::native::{
        bridge::scope::Scope,
        json as document,
        lifecycle::{manifest as manifests, state},
        resources::common,
        seed::{Registration, capacity, history, latest, runtime_model},
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(crate) struct Anchors {
    pub anchors: Value,
    pub post: Value,
    pub runtime: PathBuf,
    pub handoff: Value,
    pub environment: Value,
}

pub(super) fn collect(
    session: &Session<'_>,
    coordinator_tool: &Value,
    scope: &Scope,
    quiet: &mut dyn FnMut(&Value) -> Result<()>,
) -> Result<Anchors> {
    let Session {
        backend,
        run,
        current,
        generation,
        ..
    } = *session;
    let manifest = &session.manifest.value;
    if current == 0 {
        return Err("来源登记必须明确当前正数 attempt".into());
    }
    let before = manifests::load(backend, &run.join("manifest.json"))?;
    if before.value != *manifest || manifest["copy_stage"] != "source_to_seed" {
        return Err("只有固定完整 source_to_seed run 可登记 seed 复制源".into());
    }
    let initialized = common::bound_file(backend, &manifest["initialized"], "来源登记初始化目标")?;
    generation.protects(initialized.parent().ok_or("初始化目标缺少目录")?)?;
    let state_binding = common::binding(&run.join("state.json"))?;
    let plan = copied::require(backend, run, manifest)?;
    if plan["copy_stage"] != "source_to_seed" {
        return Err("原复制计划不是 source_to_seed".into());
    }
    let registered = Registration::load(backend, run, false)?;
    let post = history::validate(backend, run, &registered.value)?;
    let identity = identity::verified(backend, run, &registered.value, Some(current))?;
    let capacity = capacity::evidence(backend, run, &registered.value, Some(current))?;
    let inputs = runtime_model::inputs(backend, run)?;
    require_handoff(&identity, &capacity, &inputs.handoff)?;
    let saved = state::load(run, true)?;
    require_current(&saved, current)?;
    let close = close::latest(
        backend,
        run,
        &saved,
        &inputs.runtime,
        &inputs.handoff,
        current,
    )?;
    let build_binding = &post.request["backend_build"];
    let build_path = common::bound_file(backend, build_binding, "来源登记原产品构建")?;
    let build = common::read_bound(backend, build_binding, "来源登记原产品构建")?;
    let product = scope.registered_product_root(&build_path, &build)?;
    let lineage = lineage::collect(
        backend,
        run,
        coordinator_tool,
        &product,
        &saved,
        &post,
        &inputs,
    )?;
    common::bound_file(backend, build_binding, "来源登记原产品构建后像")?;
    require_quiet(&lineage, quiet)?;
    let record = latest(&saved, "seed-runtime", &["register"], None, false)?;
    let stage = attempt_binding(backend, run, record)?;
    let storage = storage::verify(backend, run, manifest, generation)?;
    let handoff = common::binding(&inputs.runtime.join("handoff.json"))?;
    if common::read_bound(backend, &handoff, "seed 固定 handoff")? != inputs.handoff {
        return Err("seed handoff 在组装期间变化".into());
    }
    let evidence = json!({"run_manifest":manifests::binding(&run.join("manifest.json"),&before.digest)?,
        "seed_registration":common::binding(&run.join("seed-runtime.json"))?,"seed_registration_stage":stage,
        "identity":identity,"capacity":capacity,"seed_handoff":handoff,"seed_close":close,"lineage":lineage,"storage":storage});
    recheck(
        backend,
        run,
        &registered,
        &before,
        &saved,
        &state_binding,
        &post,
    )?;
    common::bound_file(backend, &evidence["seed_handoff"], "seed 固定 handoff")?;
    generation.check()?;
    assemble(backend, evidence, &registered.value, post.request, inputs)
}

fn require_handoff(identity: &Value, capacity: &Value, handoff: &Value) -> Result<()> {
    document::object(identity, "身份验证证据")?;
    document::object(capacity, "容量验证证据")?;
    document::object(handoff, "seed handoff")?;
    if handoff.get("identity") != Some(identity) || handoff.get("capacity") != Some(capacity) {
        return Err("seed handoff 未绑定当前身份、部门及容量验证".into());
    }
    Ok(())
}

fn require_current(saved: &Value, current: u64) -> Result<()> {
    if current == 0 {
        return Err("来源登记当前 attempt 无效".into());
    }
    let mut selected = state::attempts(saved)?
        .iter()
        .filter(|item| item["number"].as_u64() == Some(current));
    let record = selected.next().ok_or("来源登记当前 attempt 未登记")?;
    if selected.next().is_some()
        || record["stage"] != "seed-runtime"
        || record["mode"] != "source-register"
        || record["status"] != "running"
        || state::running_attempt(saved)? != Some(record)
    {
        return Err("来源登记必须是唯一运行中的 seed-runtime/source-register attempt".into());
    }
    Ok(())
}

fn require_quiet(lineage: &Value, quiet: &mut dyn FnMut(&Value) -> Result<()>) -> Result<()> {
    for item in document::array(&lineage["identities"], "完整生产者身份")? {
        document::object(item, "生产者身份记录")?;
        let identity = item.get("identity").ok_or("历史生产者缺少完整身份")?;
        document::object(identity, "历史生产者身份")?;
        quiet(identity)?;
    }
    Ok(())
}

fn attempt_binding(backend: &Path, run: &Path, attempt: &Value) -> Result<Value> {
    let number = attempt["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("seed registration attempt 无效")?;
    let descriptor = attempt
        .get("result")
        .ok_or("seed registration 缺少外层结果绑定")?;
    if common::bound_file(backend, descriptor, "seed registration 外层结果")?
        != run.join("results").join(format!("{number:04}.json"))
    {
        return Err("seed registration 外层结果不属于固定 run attempt".into());
    }
    Ok(descriptor.clone())
}

fn recheck(
    backend: &Path,
    run: &Path,
    registered: &Registration,
    original: &manifests::Manifest,
    saved: &Value,
    binding: &Value,
    post: &history::Post,
) -> Result<()> {
    registered.assert_unchanged(backend, false)?;
    let after = manifests::load(backend, &run.join("manifest.json"))?;
    if after.value != original.value
        || after.digest != original.digest
        || state::load(run, true)? != *saved
        || common::binding(&run.join("state.json"))? != *binding
    {
        return Err("来源登记组装期间完整清单或阶段绑定变化；保留现场，禁止重放".into());
    }
    let after = history::validate(backend, run, &registered.value)?;
    if after.request != post.request
        || after.descriptor != post.descriptor
        || after.runtime != post.runtime
    {
        return Err("来源登记组装期间 post-copy 历史绑定变化".into());
    }
    common::bound_file(backend, &post.request["api_environment"], "来源环境描述")?;
    registered.assert_unchanged(backend, false)?;
    Ok(())
}

fn assemble(
    backend: &Path,
    evidence: Value,
    seed: &Value,
    post: Value,
    inputs: runtime_model::Inputs,
) -> Result<Anchors> {
    let mut public = document::exact(
        &evidence,
        &[
            "run_manifest",
            "seed_registration",
            "seed_registration_stage",
            "identity",
            "capacity",
            "seed_handoff",
            "seed_close",
            "lineage",
            "storage",
        ],
        "来源内部锚点",
    )?
    .clone();
    let environment = serde_json::from_value(inputs.environment.clone())
        .map_err(|_| "来源环境必须是固定文本映射")?;
    runtime_model::validate_api_environment(backend, &environment)?;
    for key in ["post_copy", "post_verify"] {
        public.insert(
            key.into(),
            seed.get(key).ok_or("seed 登记缺少来源绑定")?.clone(),
        );
    }
    let mut copied = serde_json::Map::new();
    for key in ["copy_stage_receipt", "copy_result", "ledger_head"] {
        copied.insert(
            key.into(),
            post.get(key).ok_or("post-copy 缺少复制锚点")?.clone(),
        );
    }
    public.insert("source_to_seed".into(), Value::Object(copied));
    public.insert(
        "departments".into(),
        evidence["identity"]
            .get("departments")
            .ok_or("身份验证缺少部门证据")?
            .clone(),
    );
    public.insert(
        "source_environment".into(),
        post.get("api_environment")
            .ok_or("post-copy 缺少来源环境绑定")?
            .clone(),
    );
    Ok(Anchors {
        anchors: Value::Object(public),
        post,
        runtime: inputs.runtime,
        handoff: inputs.handoff,
        environment: inputs.environment,
    })
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/anchors.rs"]
mod tests;
