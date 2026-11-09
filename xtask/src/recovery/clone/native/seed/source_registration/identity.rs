//! 复用完整身份账本和部门证据；不执行身份变更或重复选择历史成功。
use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        department, json as document,
        lifecycle::state,
        resources::common,
        seed::{Registration, latest},
    },
};

pub(crate) fn verified(
    backend: &Path,
    run: &Path,
    request: &Value,
    current: Option<u64>,
) -> Result<Value> {
    let registration = Registration::load(backend, run, false)?;
    if registration.value != *request {
        return Err("身份核验请求不同于当前已登记seed请求".into());
    }
    let before = state::load(run, true)?;
    let record = latest(
        &before,
        "seed-runtime",
        &["identities-apply", "identities-verify"],
        current,
        false,
    )?;
    let result = common::read_bound(backend, &record["result"], "身份核验外层结果")?;
    let plan = common::read_bound(backend, &request["identity_plan"], "身份核验计划")?;
    let root = Path::new(document::text(&request["identity_state"], "身份账本路径")?);
    let identity =
        crate::data::performance_identities::seed_evidence::verify(backend, &plan, root)?;
    let departments = department::verified_evidence(backend, run, request, current)?;
    let bound_registration = common::binding(&run.join("seed-runtime.json"))?;
    if record["mode"] != "identities-verify"
        || result.get("status").and_then(Value::as_str) != Some("seed_identities_verified")
        || result.get("registration") != Some(&bound_registration)
        || result.get("identity") != Some(&identity)
        || result.get("departments") != Some(&departments)
    {
        return Err("必须取得当前同账本identities-verify的外层通过结果".into());
    }
    let producer = result
        .get("producer")
        .ok_or("身份核验外层结果缺少producer字段")?;
    common::bound_file(backend, &request["identity_plan"], "身份核验计划")?;
    common::bound_file(backend, &record["result"], "身份核验外层结果")?;
    common::bound_file(backend, &bound_registration, "身份seed登记")?;
    if state::load(run, true)? != before {
        return Err("身份核验期间完整阶段状态变化；保留现场，禁止重放".into());
    }
    registration.assert_unchanged(backend, false)?;
    Ok(
        json!({"stage":record["result"],"identity":identity,"producer":producer,"departments":departments}),
    )
}
