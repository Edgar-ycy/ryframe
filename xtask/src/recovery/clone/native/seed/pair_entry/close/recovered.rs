//! 只接受由正式 Recover 核对并保留原停止归档的 Close 间隔。
use super::super::{Frame, Role, receipts};
use crate::{
    Result,
    recovery::clone::native::{json as document, lifecycle::state, resources::common},
};
use serde_json::{Map, Value};

pub(super) fn verify(
    frame: &Frame<'_>,
    original: &Value,
    prior: &Value,
    intervening: &[Value],
    destination: u64,
) -> Result<Vec<Value>> {
    if intervening.is_empty() {
        return Ok(Vec::new());
    }
    verify_window(frame, original, prior, intervening, destination)?;
    let mut recovered = Vec::new();
    let mut descriptors = Vec::new();
    let mut unresolved = false;
    for row in intervening {
        if receipts::recover_interrupted(row) {
            descriptors.push(receipts::recover_controller(frame, row)?);
            unresolved = true;
            continue;
        }
        let (value, row_bindings) = read_recover(frame, row)?;
        let runtime = &value["runtime"];
        for (name, role_value) in roles(runtime)? {
            let role = permitted_role(name, &prior["phase"], role_value)?;
            if recovered.contains(&role) {
                return Err("close 间隔重复核对同一角色".into());
            }
            descriptors.extend(receipts::verify_recovered_close_stop(
                frame,
                original,
                role,
                &prior["process_identities"][name],
                role_value,
                &runtime["close_stop_evidence"][name],
            )?);
            recovered.push(role);
            unresolved = false;
        }
        descriptors.extend(row_bindings);
    }
    if recovered.is_empty() || unresolved {
        return Err("close 间隔没有核对原关闭阶段的成功停止".into());
    }
    for descriptor in &descriptors {
        common::bound_file(frame.backend, descriptor, "close 间隔恢复后像")?;
    }
    frame.check()?;
    Ok(descriptors)
}

pub(super) fn candidate(row: &Value) -> bool {
    row["stage"] == "seed-runtime"
        && row["mode"] == "recover"
        && (row["status"] == "passed" || receipts::recover_interrupted(row))
}

fn verify_window(
    frame: &Frame<'_>,
    original: &Value,
    prior: &Value,
    intervening: &[Value],
    destination: u64,
) -> Result<()> {
    frame.check()?;
    if prior["handoff"] != frame.handoff {
        return Err("Recover 不属于原 Close 的固定运行交接".into());
    }
    window(
        state::attempts(&frame.saved)?,
        frame.number,
        original,
        intervening,
        destination,
    )
}

fn window(
    attempts: &[Value],
    current: u64,
    original: &Value,
    intervening: &[Value],
    destination: u64,
) -> Result<()> {
    let offset = original["number"]
        .as_u64()
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| usize::try_from(index).ok())
        .ok_or("原 close 序号无效")?;
    let end = usize::try_from(destination).map_err(|_| "目标 Close 阶段序号无效")?;
    let (next, earlier) = attempts
        .get(offset..end)
        .and_then(<[Value]>::split_last)
        .ok_or("Close 恢复区间没有固定后继阶段")?;
    let (saved, gap) = earlier.split_first().ok_or("close 间隔缺少原阶段")?;
    let status = if destination == current {
        "running"
    } else {
        "failed"
    };
    if destination > current
        || next["number"].as_u64() != Some(destination)
        || next["stage"] != "seed-runtime"
        || next["mode"] != "close"
        || next["status"] != status
        || saved != original
        || gap != intervening
        || gap.iter().any(|row| !candidate(row))
        || original["stage"] != "seed-runtime"
        || original["mode"] != "close"
        || original["status"] != "failed"
    {
        return Err("Recover 不是当前与原失败 Close 之间的同一交接".into());
    }
    Ok(())
}

fn read_recover(frame: &Frame<'_>, row: &Value) -> Result<(Value, [Value; 2])> {
    let number = row["number"].as_u64().ok_or("Recover 阶段序号无效")?;
    if row["stage"] != "seed-runtime"
        || row["mode"] != "recover"
        || row["status"] != "passed"
        || !row["error_type"].is_null()
        || row["finished_at"].as_str().is_none()
    {
        return Err("close 间隔只允许已经通过的 Seed Recover 阶段".into());
    }
    let controller = receipts::recover_controller(frame, row)?;
    let binding = &row["result"];
    let path = common::bound_file(frame.backend, binding, "原 Recover 阶段结果")?;
    if path != state::result_path(frame.run, number) {
        return Err("原 Recover 结果不同于该阶段固定位置".into());
    }
    let value = common::read_bound(frame.backend, binding, "原 Recover 阶段结果")?;
    document::exact(
        &value,
        &["status", "runtime", "restore_qualified"],
        "原 Recover 结果",
    )?;
    if value["status"] != "seed_runtime_recover" || value["restore_qualified"] != false {
        return Err("原 Recover 外层结果不同于正式只读核对".into());
    }
    Ok((value, [controller, binding.clone()]))
}

fn roles(runtime: &Value) -> Result<&Map<String, Value>> {
    let mut fields = vec![
        "format_version",
        "kind",
        "roles",
        "remote_writes_reconciled",
    ];
    if runtime.get("close_stop_evidence").is_some() {
        fields.push("close_stop_evidence");
    }
    document::exact(runtime, &fields, "原 Recover 角色结果")?;
    if runtime["format_version"].as_u64() != Some(1)
        || runtime["kind"] != "devex-clone-runtime-lock-reconciliation"
        || runtime["remote_writes_reconciled"] != false
    {
        return Err("原 Recover 角色结果没有明确零远端写入".into());
    }
    let roles = document::object(&runtime["roles"], "原 Recover 角色")?;
    if let Some(evidence) = runtime.get("close_stop_evidence") {
        let evidence = document::object(evidence, "原 Close 停止归档证据")?;
        if roles.len() != evidence.len() || roles.keys().any(|name| !evidence.contains_key(name)) {
            return Err("原 Recover 角色与原 Close 停止归档证据未逐项对应".into());
        }
    } else if !roles.is_empty() {
        return Err("原 Recover 角色没有固定的原 Close 停止归档证据".into());
    }
    Ok(roles)
}

fn permitted_role(name: &str, phase: &Value, value: &Value) -> Result<Role> {
    let role = match (name, phase.as_str()) {
        ("api", Some("running" | "api_stopped")) => Role::Api,
        ("worker", Some("drained" | "worker_stopped")) => Role::Worker,
        _ => return Err("Recover 角色不同于原 Close 精确阶段".into()),
    };
    if value["operation"] != "stop"
        || value["outcome"] != "succeeded"
        || value["state"] != "stopped"
        || value.get("identity") != Some(&Value::Null)
    {
        return Err("close 间隔只能消费成功的原角色停止结果".into());
    }
    Ok(role)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn close_recovery_window_revalidates_the_exact_historical_interval() {
        let original = json!({"number":1,"stage":"seed-runtime","mode":"close","status":"failed"});
        let recover = json!({"number":2,"stage":"seed-runtime","mode":"recover","status":"passed"});
        let interrupted =
            json!({"number":3,"stage":"seed-runtime","mode":"close","status":"failed"});
        let current = json!({"number":4,"stage":"seed-runtime","mode":"close","status":"running"});
        let rows = [original.clone(), recover.clone(), interrupted, current];
        assert!(window(&rows, 4, &original, std::slice::from_ref(&recover), 3).is_ok());
        assert!(window(&rows, 4, &original, std::slice::from_ref(&recover), 4).is_err());
        assert!(window(&rows, 4, &original, &[], 3).is_err());
        assert!(window(&rows, 4, &original, &rows[1..3], 4).is_err());
    }

    #[test]
    fn close_recovery_window_distinguishes_interruption_from_other_failures() {
        let original = json!({"number":1,"stage":"seed-runtime","mode":"close","status":"failed"});
        let interrupted = json!({"number":2,"stage":"seed-runtime","mode":"recover","status":"failed",
            "finished_at":"2026-11-09T02:00:00Z","error_type":"ControllerInterrupted","result":null});
        let passed = json!({"number":3,"stage":"seed-runtime","mode":"recover","status":"passed"});
        let current = json!({"number":4,"stage":"seed-runtime","mode":"close","status":"running"});
        let mut rows = [original.clone(), interrupted, passed, current];
        assert!(window(&rows, 4, &original, &rows[1..3], 4).is_ok());
        rows[1]["error_type"] = json!("StageError");
        assert!(window(&rows, 4, &original, &rows[1..3], 4).is_err());
        rows[1]["error_type"] = json!("ControllerInterrupted");
        rows[1]["result"] = json!({"path":"published"});
        assert!(window(&rows, 4, &original, &rows[1..3], 4).is_err());
    }

    fn runtime(roles: Value) -> Value {
        json!({"format_version":1,"kind":"devex-clone-runtime-lock-reconciliation",
            "remote_writes_reconciled":false,"roles":roles})
    }

    #[test]
    fn nonempty_recover_requires_exact_archive_evidence_keys() -> Result<()> {
        let mut empty = runtime(json!({}));
        assert!(roles(&empty)?.is_empty());
        empty["close_stop_evidence"] = json!({});
        assert!(roles(&empty)?.is_empty());
        empty["close_stop_evidence"] = Value::Null;
        assert!(roles(&empty).is_err());

        let mut stopped = runtime(json!({"api":{}}));
        assert!(roles(&stopped).is_err());
        stopped["close_stop_evidence"] = json!({"worker":{}});
        assert!(roles(&stopped).is_err());
        stopped["close_stop_evidence"] = json!({"api":{}});
        assert_eq!(roles(&stopped)?.len(), 1);
        stopped["remote_writes_reconciled"] = json!(true);
        assert!(roles(&stopped).is_err());
        Ok(())
    }

    #[test]
    fn recover_accepts_only_the_original_close_phase_and_successful_stop() -> Result<()> {
        let mut stopped = json!({"operation":"stop","outcome":"succeeded","state":"stopped",
            "identity":null});
        assert!(permitted_role("api", &json!("running"), &stopped)? == Role::Api);
        assert!(permitted_role("worker", &json!("worker_stopped"), &stopped)? == Role::Worker);
        assert!(permitted_role("worker", &json!("api_stopped"), &stopped).is_err());
        assert!(permitted_role("api", &json!("drained"), &stopped).is_err());
        assert!(permitted_role("frontend", &json!("drained"), &stopped).is_err());
        stopped["operation"] = json!("start");
        assert!(permitted_role("api", &json!("running"), &stopped).is_err());
        Ok(())
    }
}
