//! 身份准备与最终核验沿用固定账本及同一持锁 seed 阶段。

use std::path::Path;

use serde_json::{Value, json};

use super::{inspection, producer, seed_capacity::Guard};
use crate::{
    Result,
    data::performance_identities::seed_evidence,
    recovery::clone::native::{
        department, json as document, lifecycle::state, resources::common, seed::capacity,
    },
};

pub(super) fn execute(
    guard: &Guard<'_, '_>,
    request: &Value,
    registration: &Value,
) -> Result<Value> {
    let context = guard.live.context;
    let verify = match context.mode {
        "identities-apply" => false,
        "identities-verify" => true,
        _ => return Err("未知 seed 身份阶段".into()),
    };
    let departments = || {
        verify
            .then(|| {
                department::authorize_identity_verification(
                    context.backend,
                    context.run,
                    request,
                    context.number,
                )
            })
            .transpose()
    };
    let capacity =
        || capacity::evidence(context.backend, context.run, request, Some(context.number));
    let original_departments = departments()?;
    let original_capacity = capacity()?;
    guard.full()?;
    let plan = common::read_bound(context.backend, &request["identity_plan"], "seed 身份计划")?;
    let root = Path::new(document::text(&request["identity_state"], "seed 身份账本")?);
    let result = producer::with_identity(context, request, registration)?;
    require_result(&result, &plan["plan_sha256"], verify)?;
    let observe = || observe_identity(context.backend, &plan, root, verify);
    let identity = observe()?;
    guard.full()?;
    if observe()? != identity
        || departments()? != original_departments
        || capacity()? != original_capacity
    {
        return Err("seed 身份收尾期间账本、部门或容量证明变化；保留现场，禁止自动重放".into());
    }
    let saved = state::load(context.run, true)?;
    let attempt = state::running_attempt(&saved)?.ok_or("seed 身份缺少当前阶段")?;
    let observed = inspection::inspect_historical(context.backend, context.run, attempt)?;
    let kind = if verify {
        "identity-verify"
    } else {
        "identity-apply"
    };
    if attempt["number"].as_u64() != Some(context.number)
        || observed["alive"] != false
        || observed["producer_kind"] != kind
        || observed["request"] != *request
    {
        return Err("seed 身份生产者缺少完整退出证明".into());
    }
    context.check()?;
    document::write_new(&context.output.join("identity-result.json"), &result)?;
    let status = if verify {
        "seed_identities_verified"
    } else {
        "seed_identities_prepared"
    };
    let mut response = json!({"status":status,"registration":registration,"identity":identity,
        "producer":observed["receipt"],"capacity":original_capacity,
        "worker_authorized":false,"restore_qualified":false});
    if let Some(departments) = original_departments {
        response["departments"] = departments;
    }
    Ok(response)
}

fn require_result(result: &Value, digest: &Value, verify: bool) -> Result<()> {
    let expected = if verify {
        json!({"status":"verified","plan_sha256":digest,"users":200,"tenants":10,"message_audience":10})
    } else {
        json!({"status":"prepared","plan_sha256":digest,"entries":622,"roles":11,"users":200})
    };
    if *result != expected {
        return Err("seed 身份生产者结果不属于已登记的完整计划".into());
    }
    Ok(())
}

fn observe_identity(backend: &Path, plan: &Value, root: &Path, verify: bool) -> Result<Value> {
    if verify {
        return seed_evidence::verify(backend, plan, root);
    }
    let evidence = seed_evidence::projection(backend, plan, root, false)?;
    if evidence["state"] != "prepared" {
        return Err("seed 身份 apply 未形成完整 prepared 账本".into());
    }
    Ok(evidence)
}
