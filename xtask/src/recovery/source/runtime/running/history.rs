//! 完整发布后历史先验证证明任务，再解析唯一有效存储代次；不回放历史动作。
use crate::{
    Result,
    cli::CloneSide,
    recovery::clone::native::{
        cache, json as document,
        resources::{common, storage::registered},
        seed::protocol::guards,
    },
};
use serde_json::{Value, json};
use std::path::Path;

#[path = "history/completed.rs"]
mod completed;
#[path = "history/export.rs"]
mod export;
#[path = "history/lineage.rs"]
pub(in crate::recovery::source::evidence) mod lineage;
#[path = "history/prelaunch.rs"]
pub(in crate::recovery::source::evidence) mod prelaunch;
#[path = "history/reboot.rs"]
pub(in crate::recovery::source::evidence) mod reboot;
#[path = "history/resolution.rs"]
mod resolution;
#[path = "history/segment.rs"]
pub(in crate::recovery::source::evidence) mod segment;
pub(in crate::recovery::source::evidence) use segment::restart_request;
#[path = "history/stops.rs"]
mod stops;

pub(in crate::recovery::source::evidence) fn starts(
    backend: &Path,
    run: &Path,
    attempts: &[Value],
) -> Result<Vec<Value>> {
    prelaunch::starts(backend, run, attempts)
}

pub(in crate::recovery::source::evidence) fn predecessor(
    backend: &Path,
    run: &Path,
    state: &Value,
) -> Result<Value> {
    let selected = super::facts::model("select-rebind", json!({"attempts":state["attempts"]}))?;
    let result = common::read_bound(backend, &selected["record"]["result"], "来源重绑定结果")?;
    let mut source = cache::binding::source::load(backend, &result["review_successor"])?;
    if source["directory"] != json!(run) {
        return Err("源运行代次不属于同一原复制账本".into());
    }
    let descriptor = &result["source_registration"];
    let history = resolution::inspect(backend, run, state, descriptor)?;
    let selected = model(
        "resolve-plan",
        json!({"attempts":state["attempts"],"later":history["later"]}),
    )?;
    let original = &source["storage"]["storage"];
    let (rebound, receipt, registered) = rebound(
        backend,
        run,
        &selected,
        descriptor,
        original,
        &history["segment"],
    )?;
    let segment = if history["segment"].is_null() {
        Value::Null
    } else {
        json!({"storage":history["segment"]["storage"],"phase":history["segment"]["phase"]})
    };
    let effective = model(
        "resolve-storage",
        json!({"original":original,"rebound":rebound,
        "registered":registered,"segment":segment,"live":false,"current":null}),
    )?;
    source["storage"]["storage"] = effective["effective"].clone();
    source["source_rebind"] = receipt;
    Ok(source)
}

fn rebound(
    backend: &Path,
    run: &Path,
    selected: &Value,
    descriptor: &Value,
    original: &Value,
    segment: &Value,
) -> Result<(Value, Value, Value)> {
    let record = &selected["record"];
    if record.is_null() {
        return Ok((Value::Null, Value::Null, Value::Null));
    }
    let number = record["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("重绑定序号无效")?;
    let receipt = &record["result"];
    if common::bound_file(backend, receipt, "重绑定结果")?
        != run.join(format!("results/{number:04}.json"))
    {
        return Err("重绑定结果不属于固定 run attempt".into());
    }
    let value = common::read_bound(backend, receipt, "重绑定结果")?;
    guards::rebind_receipt(&json!({"value":value,"descriptor":descriptor,
        "original":original,"prefix":selected["prefix"]}))?;
    common::bound_file(backend, &value["review_successor"], "重绑定审阅后继")?;
    guards::rebind::live::transition(
        backend,
        &json!({"original":original,"current":value["current_storage"]}),
    )?;
    let registered = if segment.is_null() {
        registered::registered(backend, run, CloneSide::Target, None)?.unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    common::bound_file(backend, receipt, "重绑定结果")?;
    Ok((
        value["current_storage"].clone(),
        receipt.clone(),
        registered,
    ))
}

fn model(action: &str, input: Value) -> Result<Value> {
    let result = guards::rebind::dispatch(&json!({"action":action,"input":input}))?;
    if let Some(reason) = result.get("rejected") {
        return Err(document::text(reason, "重绑定拒绝原因")?.to_owned().into());
    }
    Ok(result)
}
