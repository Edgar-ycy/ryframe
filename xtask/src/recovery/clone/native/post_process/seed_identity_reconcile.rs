//! 正式身份创建和解只读原请求与实际后像，再发布有限的账本确认和后继工具。

use std::path::Path;

use serde_json::{Value, json};

use super::{context::Context, inspection, seed_capacity::Guard};
use crate::{
    Result,
    data::performance_identities::reconcile,
    recovery::clone::native::{
        json as document,
        lifecycle::state,
        resources::common,
        seed::{capacity, identity_successor},
        target_verify::guard::Inputs,
    },
};

#[path = "seed_identity_reconcile/trace.rs"]
mod trace;

pub(super) fn execute(
    guard: &Guard<'_, '_>,
    seed: &Value,
    registration: &Value,
    request: &Path,
) -> Result<Value> {
    let context = guard.live.context;
    let descriptor = common::binding(request)?;
    let request = common::read_bound(context.backend, &descriptor, "身份和解请求")?;
    if request["kind"] == super::identity_session_reconcile::REQUEST_KIND {
        return super::identity_session_reconcile::execute(guard, seed, &descriptor, &request);
    }
    validate_request(&request, registration)?;
    let prior = prior_attempt(context.run, context.number, &request)?;
    let observed = original_producer(context, &prior, &request, seed)?;
    let inputs = inputs(context.backend, &descriptor, &request, &observed)?;
    let plan = common::read_bound(context.backend, &seed["identity_plan"], "原身份计划")?;
    let root = Path::new(document::text(&seed["identity_state"], "原身份账本路径")?);
    reconcile::original_lock_release(
        context.backend,
        root,
        &request["lock_release"],
        &observed["value"],
        &plan,
    )?;
    let active_api = common::read_bound(context.backend, &seed["api_process"], "原 API 进程收据")?;
    let build = bound_tool(context.backend, seed, &request)?;
    let initial_capacity =
        capacity::evidence(context.backend, context.run, seed, Some(context.number))?;
    let schedules = guard.full()?;
    let inspect_trace = || {
        trace::read(
            context.backend,
            &context.post.runtime,
            &active_api["identity"],
            &context.post.request["source_admin"],
            &prior,
            document::text(&request["log_line_sha256"], "原 HTTP 跟踪摘要")?,
        )
    };
    let trace = inspect_trace()?;
    document::write_new(
        &context.output.join("identity-reconciliation-trace.json"),
        &trace.evidence,
    )?;
    let authority = json!({"request":descriptor,"original_failure":request["failure"],
        "original_producer":request["producer"],"original_lock_release":request["lock_release"],"trace":trace.evidence});
    let check = &mut || {
        inputs.check()?;
        guard.quick(&schedules)?;
        let current = inspect_trace()?;
        if current.evidence != trace.evidence || current.audit != trace.audit {
            return Err("身份和解核对期间原 HTTP 跟踪或资源归属变化".into());
        }
        Ok(())
    };
    reconcile::first_create(
        &plan,
        root,
        document::text(&request["ledger_sha256"], "原身份账本摘要")?,
        &authority,
        &trace.audit,
        check,
    )?;
    guard.full()?;
    if capacity::evidence(context.backend, context.run, seed, Some(context.number))?
        != initial_capacity
    {
        return Err("身份和解期间原容量证据改变".into());
    }
    check()?;
    let receipt = common::binding(
        &root.join("first-create-reconciliation/first-create-reconciliation-result.json"),
    )?;
    identity_successor::publish(
        context.backend,
        context.run,
        registration,
        &build,
        &request["build"],
        &receipt,
    )
}

fn original_producer(
    context: &Context<'_>,
    prior: &Value,
    request: &Value,
    seed: &Value,
) -> Result<Value> {
    let observed = inspection::inspect_historical(context.backend, context.run, prior)?;
    if observed["alive"] != false
        || observed["producer_kind"] != "identity-apply"
        || observed["request"] != *seed
        || observed["receipt"] != request["producer"]
    {
        return Err("身份和解缺少已退出的原身份生产者证明".into());
    }
    Ok(observed)
}

fn validate_request(request: &Value, registration: &Value) -> Result<()> {
    document::exact(
        request,
        &[
            "format_version",
            "kind",
            "registration",
            "failure",
            "producer",
            "lock_release",
            "ledger_sha256",
            "log_line_sha256",
            "build",
        ],
        "身份和解请求",
    )?;
    if request["format_version"] != 1
        || request["kind"] != "devex-seed-identity-first-create-reconciliation"
        || request["registration"] != *registration
    {
        return Err("身份和解请求不属于原 seed 登记".into());
    }
    Ok(())
}

fn prior_attempt(run: &Path, current: u64, request: &Value) -> Result<Value> {
    let saved = state::load(run, true)?;
    let attempts = state::attempts(&saved)?;
    let previous = attempts
        .iter()
        .rposition(|attempt| {
            attempt["stage"] == "seed-runtime"
                && attempt["mode"] == "identities-apply"
                && attempt["number"]
                    .as_u64()
                    .is_some_and(|number| number < current)
        })
        .ok_or("身份和解缺少原身份应用阶段")?;
    let original = &attempts[previous];
    let number = original["number"]
        .as_u64()
        .ok_or("原身份应用阶段编号无效")?;
    if original["status"] != "failed"
        || original["error_type"] != "StageError"
        || original["result"] != Value::Null
        || attempts[..previous].iter().any(|attempt| {
            attempt["stage"] == "seed-runtime"
                && matches!(
                    attempt["mode"].as_str(),
                    Some("identities-apply" | "identities-verify" | identity_successor::MODE)
                )
        })
        || attempts[previous + 1..].iter().any(|attempt| {
            attempt["stage"] != "seed-runtime"
                || attempt["mode"] != identity_successor::MODE
                || (attempt["number"].as_u64() != Some(current) && attempt["status"] != "failed")
        })
    {
        return Err("身份和解只接受首次身份应用的原失败与后续同职责尝试".into());
    }
    let failure = run.join(format!("failure-{number:04}.json"));
    let controller = state::controller_path(run, number);
    if common::binding(&failure)? != request["failure"]
        || document::read(&failure)?
            != json!({
                "format_version":2,"kind":"devex-stage-failure","attempt":number,
                "stage":"seed-runtime","mode":"identities-apply","error_type":"StageError",
                "operation_id":"clone.seed-runtime.identities-apply","controller":common::binding(&controller)?,
            })
    {
        return Err("身份和解未绑定原失败和原阶段控制器".into());
    }
    Ok(original.clone())
}

fn inputs(backend: &Path, descriptor: &Value, request: &Value, observed: &Value) -> Result<Inputs> {
    let mut files = vec![common::bound_file(backend, descriptor, "身份和解请求")?];
    for name in [
        "registration",
        "failure",
        "producer",
        "lock_release",
        "build",
    ] {
        files.push(common::bound_file(
            backend,
            &request[name],
            "身份和解授权原件",
        )?);
    }
    files.push(common::bound_file(
        backend,
        &observed["value"]["launch"],
        "原身份生产者启动",
    )?);
    Inputs::open(files)
}

fn bound_tool(backend: &Path, seed: &Value, request: &Value) -> Result<Value> {
    let current = crate::rate_limit::current_tool_binding()?;
    let current = json!({"path":current["path"],"sha256":current["sha256"]});
    let evidence = common::read_bound(backend, &request["build"], "和解工具独立构建收据")?;
    if evidence["format_version"] != 1
        || evidence["kind"] != "devex-seed-identity-successor-build"
        || evidence["tool"] != current
        || seed["identity_tool"] == current
    {
        return Err("当前身份和解控制器与独立后继构建绑定不同".into());
    }
    Ok(current)
}
