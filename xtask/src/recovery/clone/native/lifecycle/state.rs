use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::{
    digest,
    json::{self as json_io, array, exact, object, text},
};
use super::manifest;
use crate::Result;

pub(crate) mod prefix;

pub(crate) const STAGES: &[&str] = &[
    "export",
    "target-verify",
    "target-cache-markers",
    "copy",
    "runtime-source",
    "runtime-target",
    "post-copy",
    "seed-runtime",
    "storage-source",
    "storage-target",
    "cache-target",
    "fixture-buckets",
    "fixture-services",
];

pub(super) fn resource_cleanup(stage: &str, mode: &str) -> bool {
    matches!(stage, "cache-target" | "runtime-target" | "runtime-source")
        && matches!(mode, "stop" | "recover")
        || stage == "seed-runtime" && matches!(mode, "stop" | "recover")
        || matches!(stage, "post-copy" | "seed-runtime") && mode == "recover-session"
}

pub(crate) fn load(directory: &Path, verify_results: bool) -> Result<Value> {
    let value = json_io::read(&directory.join("state.json"))?;
    let root = exact(
        &value,
        &["format_version", "manifest", "attempts"],
        "验收阶段记录",
    )?;
    if root.get("format_version").and_then(Value::as_u64) != Some(1) {
        return Err("验收阶段记录类型无效".into());
    }
    let expected = manifest_binding(directory)?;
    if root.get("manifest") != Some(&expected) {
        return Err("固定验收清单发生变化".into());
    }
    validate_attempts(
        array(root.get("attempts").unwrap(), "验收阶段记录")?,
        directory,
        verify_results,
    )?;
    Ok(value)
}

pub(crate) fn initialize(directory: &Path) -> Result<()> {
    fs::create_dir(directory.join("results"))
        .map_err(|error| format!("无法创建阶段结果目录：{error}"))?;
    json_io::write_new(
        &directory.join("state.json"),
        &json!({
            "format_version": 1,
            "manifest": manifest_binding(directory)?,
            "attempts": [],
        }),
    )
}

pub(crate) fn begin(
    directory: &Path,
    stage: &str,
    mode: &str,
    sources: &Value,
) -> Result<(u64, Value)> {
    if !STAGES.contains(&stage) {
        return Err("未知验收阶段".into());
    }
    let mut value = load(directory, !resource_cleanup(stage, mode))?;
    let attempts = value["attempts"].as_array_mut().unwrap();
    if attempts
        .iter()
        .any(|attempt| attempt.get("status").and_then(Value::as_str) == Some("running"))
    {
        return Err("存在未收尾阶段，须先核实运行进程并显式恢复控制锁".into());
    }
    let number = u64::try_from(attempts.len())
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or("验收阶段序号溢出")?;
    let attempt = json!({
        "number": number,
        "stage": stage,
        "mode": mode,
        "started_at": now(),
        "finished_at": Value::Null,
        "status": "running",
        "sources": sources,
        "result": Value::Null,
        "error_type": Value::Null,
    });
    attempts.push(attempt.clone());
    write(directory, &value)?;
    Ok((number, attempt))
}

pub(crate) fn bind_controller(
    directory: &Path,
    number: u64,
    owner: &Value,
    attempt: &Value,
) -> Result<()> {
    let value = load(directory, false)?;
    let last = value["attempts"]
        .as_array()
        .and_then(|items| items.last())
        .ok_or("验收阶段记录为空")?;
    if last != attempt
        || last.get("number").and_then(Value::as_u64) != Some(number)
        || last.get("status").and_then(Value::as_str) != Some("running")
        || json_io::read(&directory.join("run.lock/owner.json"))? != *owner
    {
        return Err("只能绑定当前持锁控制器刚开始的阶段".into());
    }
    if !resource_cleanup(
        json_io::text(&last["stage"], "阶段")?,
        json_io::text(&last["mode"], "模式")?,
    ) {
        load(directory, true)?;
    }
    json_io::write_new(
        &controller_path(directory, number),
        &json!({
            "format_version": 1,
            "kind": "devex-stage-controller",
            "owner": owner,
            "attempt": number,
            "attempt_sha256": digest::plan_hash(attempt)?,
        }),
    )
}

pub(crate) fn finish(
    directory: &Path,
    number: u64,
    result: Option<&Value>,
    error_type: Option<&str>,
) -> Result<()> {
    let mut value = load(directory, false)?;
    let current = running_attempt(&value)?.ok_or("不能收尾不存在的运行阶段")?;
    if !resource_cleanup(
        json_io::text(&current["stage"], "阶段")?,
        json_io::text(&current["mode"], "模式")?,
    ) {
        value = load(directory, true)?;
    }
    let attempts = value["attempts"].as_array_mut().unwrap();
    let last = attempts.last_mut().ok_or("不能收尾不存在的阶段")?;
    if last.get("number").and_then(Value::as_u64) != Some(number)
        || last.get("status").and_then(Value::as_str) != Some("running")
    {
        return Err("不能收尾其他执行者或已经结束的阶段".into());
    }
    if let Some(result) = result {
        let path = result_path(directory, number);
        if path.exists() {
            if digest::linked(
                &fs::symlink_metadata(&path)
                    .map_err(|error| format!("无法核验已保存阶段结果：{error}"))?,
            ) || json_io::read(&path)? != *result
            {
                return Err("已保存的阶段结果不同，拒绝覆盖失败证据".into());
            }
        } else {
            json_io::write_new(&path, result)?;
        }
        last["result"] = manifest::binding(&path, &digest::file(&path)?)?;
    }
    if error_type.is_none() && result.is_none() {
        return Err("成功阶段必须发布实际结果".into());
    }
    last["status"] = Value::String(if error_type.is_some() {
        "failed".into()
    } else {
        "passed".into()
    });
    last["finished_at"] = Value::String(now());
    last["error_type"] = error_type.map_or(Value::Null, |value| Value::String(value.into()));
    write(directory, &value)
}

pub(crate) fn record_failure(
    directory: &Path,
    number: u64,
    stage: &str,
    mode: &str,
    error_type: &str,
) -> Result<()> {
    let controller = controller_path(directory, number);
    json_io::write_new(
        &directory.join(format!("failure-{number:04}.json")),
        &json!({
            "format_version": 2,
            "kind": "devex-stage-failure",
            "attempt": number,
            "stage": stage,
            "mode": mode,
            "error_type": error_type,
            "operation_id": format!("clone.{stage}.{mode}"),
            "controller": manifest::binding(&controller, &digest::file(&controller)?)?,
        }),
    )
}

pub(crate) fn running_attempt(value: &Value) -> Result<Option<&Value>> {
    let mut running = array(
        value.get("attempts").ok_or("阶段记录缺少 attempts")?,
        "验收阶段记录",
    )?
    .iter()
    .filter(|attempt| attempt.get("status").and_then(Value::as_str) == Some("running"));
    let first = running.next();
    if running.next().is_some() {
        return Err("不能把多个运行中阶段归属给同一控制器".into());
    }
    Ok(first)
}

pub(crate) fn attempts(value: &Value) -> Result<&[Value]> {
    array(
        value.get("attempts").ok_or("阶段记录缺少 attempts")?,
        "验收阶段记录",
    )
}

pub(crate) fn historical(
    directory: &Path,
    descriptor: &Value,
    verify_results: bool,
) -> Result<Value> {
    let descriptor = exact(descriptor, &["path", "bytes", "sha256"], "历史阶段记录描述")?;
    let path = Path::new(text(descriptor.get("path").unwrap(), "历史阶段记录路径")?);
    if path != directory.join("state.json")
        || descriptor
            .get("bytes")
            .and_then(Value::as_u64)
            .is_none_or(|bytes| bytes == 0)
    {
        return Err("历史阶段记录描述不属于固定运行目录".into());
    }
    let expected_sha = text(descriptor.get("sha256").unwrap(), "历史阶段记录摘要")?;
    if expected_sha.len() != 64 || !expected_sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("历史阶段记录摘要无效".into());
    }
    let before = digest::file(path)?;
    let value = load(directory, verify_results)?;
    let raw = fs::read(path)?;
    if raw.len() as u64 != before.bytes
        || hex::encode(Sha256::digest(&raw)) != before.sha256
        || serde_json::from_slice::<Value>(&raw)? != value
    {
        return Err("阶段记录在历史前缀核对期间变化".into());
    }
    let historical_attempts = prefix::count(
        &raw,
        descriptor.get("bytes").and_then(Value::as_u64).unwrap(),
        expected_sha,
    )?;
    if digest::file(path)? != before {
        return Err("阶段记录在历史前缀核对期间变化".into());
    }
    Ok(json!({
        "state": value,
        "historical_attempts": historical_attempts,
        "current": manifest::binding(path, &before)?,
    }))
}

pub(crate) fn interrupt_owned(
    directory: &Path,
    owner: &Value,
    controller_number: Option<u64>,
    prewrite: impl FnOnce(&Value) -> Result<()>,
) -> Result<Value> {
    let mut value = load(directory, false)?;
    let attempts = value["attempts"]
        .as_array_mut()
        .ok_or("阶段记录 attempts 类型无效")?;
    let mut changed = false;
    for attempt in attempts {
        if attempt.get("status").and_then(Value::as_str) != Some("running") {
            continue;
        }
        let number = attempt
            .get("number")
            .and_then(Value::as_u64)
            .ok_or("运行中阶段序号无效")?;
        let controller = controller_path(directory, number);
        if controller.exists() {
            let (_, recorded_owner) = controller_record(directory, number, attempt)?;
            if recorded_owner != *owner {
                continue;
            }
        }
        if controller_number.is_none_or(|expected| expected == number) {
            attempt["status"] = Value::String("failed".into());
            attempt["finished_at"] = Value::String(now());
            attempt["error_type"] = Value::String("ControllerInterrupted".into());
            changed = true;
        }
    }
    if !changed {
        return Err("死亡控制器没有可恢复的运行中阶段".into());
    }
    prewrite(&value)?;
    write(directory, &value)?;
    Ok(value)
}

pub(crate) fn manifest_binding(directory: &Path) -> Result<Value> {
    let path = directory.join("manifest.json");
    manifest::binding(&path, &digest::file(&path)?)
}

pub(crate) fn controller_path(directory: &Path, number: u64) -> PathBuf {
    directory.join(format!("controller-{number:04}.json"))
}

pub(crate) fn result_path(directory: &Path, number: u64) -> PathBuf {
    directory.join("results").join(format!("{number:04}.json"))
}

pub(crate) fn controller_record(
    directory: &Path,
    number: u64,
    attempt: &Value,
) -> Result<(Value, Value)> {
    let path = controller_path(directory, number);
    let binding = manifest::binding(&path, &digest::file(&path)?)?;
    let value = json_io::read(&path)?;
    let root = exact(
        &value,
        &[
            "format_version",
            "kind",
            "owner",
            "attempt",
            "attempt_sha256",
        ],
        "持久控制器",
    )?;
    if root.get("format_version").and_then(Value::as_u64) != Some(1)
        || root.get("kind").and_then(Value::as_str) != Some("devex-stage-controller")
        || root.get("attempt").and_then(Value::as_u64) != Some(number)
        || root.get("attempt_sha256").and_then(Value::as_str)
            != Some(digest::plan_hash(attempt)?.as_str())
    {
        return Err("持久控制器与当前阶段完整记录不同".into());
    }
    Ok((binding, root.get("owner").unwrap().clone()))
}

fn validate_attempts(attempts: &[Value], directory: &Path, verify_results: bool) -> Result<()> {
    for (index, attempt) in attempts.iter().enumerate() {
        let attempt = exact(
            attempt,
            &[
                "number",
                "stage",
                "mode",
                "started_at",
                "finished_at",
                "status",
                "sources",
                "result",
                "error_type",
            ],
            "验收阶段记录",
        )?;
        let number = u64::try_from(index + 1).map_err(|_| "验收阶段序号溢出")?;
        let stage = text(attempt.get("stage").unwrap(), "阶段")?;
        let status = text(attempt.get("status").unwrap(), "阶段状态")?;
        if attempt.get("number").and_then(Value::as_u64) != Some(number)
            || !STAGES.contains(&stage)
            || !matches!(status, "running" | "passed" | "failed")
            || status == "passed" && attempt.get("result").unwrap().is_null()
        {
            return Err("验收阶段顺序或状态无效".into());
        }
        text(attempt.get("mode").unwrap(), "阶段模式")?;
        text(attempt.get("started_at").unwrap(), "阶段开始时间")?;
        if !attempt.get("finished_at").unwrap().is_null() {
            text(attempt.get("finished_at").unwrap(), "阶段结束时间")?;
        }
        object(attempt.get("sources").unwrap(), "阶段来源")?;
        if !attempt.get("error_type").unwrap().is_null() {
            text(attempt.get("error_type").unwrap(), "阶段错误类型")?;
        }
        if !attempt.get("result").unwrap().is_null() {
            validate_result(
                attempt.get("result").unwrap(),
                directory,
                number,
                verify_results,
            )?;
        }
    }
    Ok(())
}

fn validate_result(
    value: &Value,
    directory: &Path,
    number: u64,
    verify_results: bool,
) -> Result<()> {
    let result = exact(value, &["path", "bytes", "sha256"], "验收阶段结果")?;
    let path = Path::new(text(result.get("path").unwrap(), "结果路径")?);
    if path != result_path(directory, number)
        || result
            .get("bytes")
            .and_then(Value::as_u64)
            .is_none_or(|value| value == 0)
        || result
            .get("sha256")
            .and_then(Value::as_str)
            .is_none_or(|value| !is_sha256(value))
        || verify_results && manifest::binding(path, &digest::file(path)?)? != *value
    {
        return Err("验收阶段结果缺失或被修改".into());
    }
    Ok(())
}

fn write(directory: &Path, value: &Value) -> Result<()> {
    json_io::write_replace(&directory.join("state.json"), value)
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
