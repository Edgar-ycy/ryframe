//! 持久绑定已核对的原停止归档，允许显式 Recover 继续核验中断记录。

use super::{Frame, Role, directory};
use crate::{
    Result,
    recovery::clone::native::{
        json as document,
        lifecycle::{identity, state},
        resources::common,
        target_verify::publication,
    },
};
use serde_json::{Value, json};
use std::{fs, path::Path};

const KIND: &str = "devex-seed-close-stop-projection";

pub(super) fn record(frame: &Frame<'_>, role: Role, evidence: &Value) -> Result<Value> {
    check_role(role)?;
    frame.check()?;
    let _ = current_tail(frame)?;
    require_earlier_close(evidence, frame.number)?;
    check_root(frame)?;
    let output = directory(frame.run, frame.number);
    if !present(&output)? {
        fs::create_dir(&output)?;
    }
    let before = directory_identity(frame, &output)?;
    let path = output.join(filename(role));
    let value = marker(frame, frame.number, &frame.controller, role, evidence);
    publication::publish(&path, &value)?;
    let descriptor = checked_marker(frame, &path, &value)?;
    if directory_identity(frame, &output)? != before {
        return Err("当前 Recover 标记目录在发布期间变化".into());
    }
    frame.check()?;
    Ok(descriptor)
}

pub(super) fn prior(frame: &Frame<'_>, role: Role) -> Result<Option<(Value, Value)>> {
    check_role(role)?;
    frame.check()?;
    check_root(frame)?;
    let tail = current_tail(frame)?;
    for row in tail.iter().rev().take_while(|row| interrupted(row)) {
        let number = number(row)?;
        let output = directory(frame.run, number);
        if !present(&output)? {
            continue;
        }
        let before = directory_identity(frame, &output)?;
        let path = output.join(filename(role));
        if !present(&path)? {
            continue;
        }
        let controller = controller_binding(frame, row)?;
        let descriptor = common::binding(&common::common_file(
            frame.backend,
            &path,
            "原 Recover 停止标记",
        )?)?;
        let saved = common::read_bound(frame.backend, &descriptor, "原 Recover 停止标记")?;
        document::exact(
            &saved,
            &[
                "format_version",
                "kind",
                "attempt",
                "controller",
                "handoff",
                "role",
                "evidence",
            ],
            "原 Recover 停止标记",
        )?;
        let evidence = &saved["evidence"];
        require_earlier_close(evidence, number)?;
        let expected = marker(frame, number, &controller, role, evidence);
        if saved != expected
            || checked_marker(frame, &path, &expected)? != descriptor
            || directory_identity(frame, &output)? != before
        {
            return Err("原 Recover 停止标记不同于同一控制器、角色和运行交接".into());
        }
        frame.check()?;
        common::bound_file(frame.backend, &descriptor, "原 Recover 停止标记后像")?;
        if directory_identity(frame, &output)? != before {
            return Err("原 Recover 停止标记目录在后验期间变化".into());
        }
        return Ok(Some((evidence.clone(), descriptor)));
    }
    frame.check()?;
    Ok(None)
}

pub(in crate::recovery::clone::native::seed::pair_entry) fn interrupted(row: &Value) -> bool {
    row["stage"] == "seed-runtime"
        && row["mode"] == "recover"
        && row["status"] == "failed"
        && row["error_type"] == "ControllerInterrupted"
        && row.get("result") == Some(&Value::Null)
        && row["finished_at"].as_str().is_some()
}

pub(in crate::recovery::clone::native::seed::pair_entry) fn controller_binding(
    frame: &Frame<'_>,
    row: &Value,
) -> Result<Value> {
    let number = number(row)?;
    let index = usize::try_from(number - 1)?;
    if row["stage"] != "seed-runtime"
        || row["mode"] != "recover"
        || number >= frame.number
        || state::attempts(&frame.saved)?.get(index) != Some(row)
    {
        return Err("原控制器不属于当前状态中的历史 Seed Recover".into());
    }
    let mut running = row.clone();
    running["status"] = json!("running");
    for field in ["finished_at", "result", "error_type"] {
        running[field] = Value::Null;
    }
    let (controller, owner) = state::controller_record(frame.run, number, &running)?;
    identity::validate(&owner, frame.run)?;
    if owner["manifest_sha256"] != state::manifest_binding(frame.run)?["sha256"] {
        return Err("原 Recover 控制器不同于固定运行清单".into());
    }
    common::bound_file(frame.backend, &controller, "原 Recover 控制器")?;
    Ok(controller)
}

fn current_tail<'a>(frame: &'a Frame<'_>) -> Result<&'a [Value]> {
    let (current, tail) = state::attempts(&frame.saved)?
        .split_last()
        .ok_or("停止归档标记缺少当前 Recover 阶段")?;
    if current["number"].as_u64() != Some(frame.number)
        || current["stage"] != "seed-runtime"
        || current["mode"] != "recover"
        || current["status"] != "running"
    {
        return Err("停止归档标记只允许当前运行中的 Seed Recover".into());
    }
    Ok(tail)
}

fn check_role(role: Role) -> Result<()> {
    if !matches!(role, Role::Api | Role::Worker) {
        return Err("停止归档标记只接受明确的 API 或 Worker".into());
    }
    Ok(())
}

fn number(row: &Value) -> Result<u64> {
    row["number"]
        .as_u64()
        .filter(|value| *value > 0)
        .ok_or_else(|| "Recover 阶段序号无效".into())
}

fn require_earlier_close(evidence: &Value, recover: u64) -> Result<()> {
    if !evidence["close_attempt"]
        .as_u64()
        .is_some_and(|close| close > 0 && close < recover)
    {
        return Err("停止归档标记缺少严格早于保存阶段的原 Close 序号".into());
    }
    Ok(())
}

fn check_root(frame: &Frame<'_>) -> Result<()> {
    common::local_directory(
        frame.backend,
        &frame.run.join("seed-runtime"),
        "seed Recover 根目录",
    )?;
    Ok(())
}

fn directory_identity(frame: &Frame<'_>, path: &Path) -> Result<(u64, u64)> {
    let path = common::local_directory(frame.backend, path, "seed Recover 标记目录")?;
    common::device_inode_for(&path, &fs::symlink_metadata(&path)?)
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn filename(role: Role) -> String {
    format!("close-stop-projection-{}.json", role.name())
}

fn marker(
    frame: &Frame<'_>,
    number: u64,
    controller: &Value,
    role: Role,
    evidence: &Value,
) -> Value {
    json!({"format_version":1,"kind":KIND,"attempt":number,"controller":controller,
        "handoff":frame.handoff,"role":role.name(),"evidence":evidence})
}

fn checked_marker(frame: &Frame<'_>, path: &Path, value: &Value) -> Result<Value> {
    let path = common::common_file(frame.backend, path, "seed Recover 停止标记")?;
    let descriptor = common::binding(&path)?;
    let mut expected = serde_json::to_vec_pretty(value)?;
    expected.push(b'\n');
    if fs::read(&path)? != expected {
        return Err("seed Recover 停止标记的原始字节不同".into());
    }
    common::bound_file(frame.backend, &descriptor, "seed Recover 停止标记后像")?;
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_finished_interrupted_recover_without_a_published_result_is_selected() {
        let row = json!({"number":2,"stage":"seed-runtime","mode":"recover","status":"failed",
            "error_type":"ControllerInterrupted","result":null,"finished_at":"fixture"});
        assert!(interrupted(&row));
        for (field, value) in [
            ("stage", json!("post-copy")),
            ("mode", json!("close")),
            ("status", json!("passed")),
            ("error_type", json!("Unknown")),
            ("result", json!({"path":"fixture"})),
            ("finished_at", Value::Null),
        ] {
            let mut altered = row.clone();
            altered[field] = value;
            assert!(!interrupted(&altered));
        }
        let mut missing_result = row;
        missing_result.as_object_mut().unwrap().remove("result");
        assert!(!interrupted(&missing_result));
    }

    #[test]
    fn projection_requires_a_strictly_earlier_positive_close() {
        assert!(require_earlier_close(&json!({"close_attempt":1}), 2).is_ok());
        for attempt in [json!(0), json!(2), json!(3), json!("1"), Value::Null] {
            assert!(require_earlier_close(&json!({"close_attempt":attempt}), 2).is_err());
        }
    }
}
