//! 部门采用链中的控制器、失败节点与构建指纹校验。
use super::{common, digest, document};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn fingerprints(attempt: &Value) -> Result<Value> {
    let sources = &attempt["sources"];
    document::exact(
        sources,
        &["snapshot", "worktree_fingerprint", "fingerprints"],
        "部门执行来源",
    )?;
    let values = &sources["fingerprints"];
    document::exact(
        values,
        &["product", "test_tools", "support"],
        "部门来源指纹",
    )?;
    let hash = regex::Regex::new(r"^[a-f0-9]{64}$")?;
    for (_, value) in document::object(values, "部门来源指纹")? {
        document::exact(value, &["sha256", "files"], "部门来源摘要")?;
        if !hash.is_match(document::text(&value["sha256"], "部门来源摘要")?)
            || value["files"].as_u64().filter(|n| *n > 0).is_none()
        {
            return Err("部门计划采用的执行来源摘要无效".into());
        }
    }
    Ok(values.clone())
}

pub(super) fn controller(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    current: bool,
    processes: &Value,
) -> Result<(Value, Value)> {
    let number = attempt["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("部门控制器序号无效")?;
    let path = run.join(format!("controller-{number:04}.json"));
    let descriptor = common::binding(&path)?;
    let value = common::read_bound(backend, &descriptor, "部门控制器")?;
    document::exact(
        &value,
        &[
            "format_version",
            "kind",
            "owner",
            "attempt",
            "attempt_sha256",
        ],
        "部门控制器",
    )?;
    let owner = &value["owner"];
    document::exact(
        owner,
        &["format_version", "identity", "directory", "manifest_sha256"],
        "部门控制器 owner",
    )?;
    let identity = &owner["identity"];
    document::exact(
        identity,
        &["pid", "started", "executable"],
        "部门控制器身份",
    )?;
    let pid = identity["pid"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("部门控制器 PID 无效")?;
    let started = generation(&identity["started"])?;
    if document::text(&identity["executable"], "部门控制器程序")?.is_empty() {
        return Err("部门控制器程序为空".into());
    }
    let mut original = attempt.clone();
    for (key, item) in [
        ("status", json!("running")),
        ("finished_at", Value::Null),
        ("result", Value::Null),
        ("error_type", Value::Null),
    ] {
        original[key] = item;
    }
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-stage-controller"
        || value["attempt"].as_u64() != Some(number)
        || value["attempt_sha256"] != digest::plan_hash(&original)?
        || owner["format_version"].as_u64() != Some(1)
        || owner["directory"] != run.to_string_lossy().as_ref()
        || owner["manifest_sha256"] != common::binding(&run.join("manifest.json"))?["sha256"]
    {
        return Err("部门计划控制器未绑定原始 attempt 和固定 manifest".into());
    }
    let actual = processes
        .get(pid.to_string())
        .ok_or("部门控制器缺少本次内核观察")?;
    if current {
        if common::read_bound(
            backend,
            &common::binding(&run.join("run.lock/owner.json"))?,
            "部门运行锁",
        )? != *owner
            || actual != identity
        {
            return Err("当前部门计划采用未绑定存活持锁控制器".into());
        }
    } else if !actual.is_null() {
        document::exact(actual, &["pid", "started", "executable"], "原部门进程观察")?;
        if actual["pid"].as_u64() != Some(pid) || generation(&actual["started"])? <= started {
            return Err("原部门控制进程仍存在或内核身份不明".into());
        }
    }
    common::bound_file(backend, &descriptor, "部门控制器")?;
    Ok((descriptor, value))
}

fn generation(value: &Value) -> Result<u128> {
    let text = document::text(value, "部门进程代次")?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err("部门进程代次无效".into());
    }
    text.parse().map_err(|_| "部门进程代次超出范围".into())
}

pub(super) fn failure(
    backend: &Path,
    run: &Path,
    attempt: &Value,
    controller: &Value,
    controller_value: &Value,
) -> Result<(Value, Value)> {
    let number = attempt["number"].as_u64().ok_or("部门失败序号无效")?;
    let descriptor = common::binding(&run.join(format!("failure-{number:04}.json")))?;
    let value = common::read_bound(backend, &descriptor, "部门失败收据")?;
    let mut fields = vec![
        "format_version",
        "kind",
        "attempt",
        "stage",
        "mode",
        "error_type",
        "controller",
    ];
    match value["format_version"].as_u64() {
        Some(1) => {
            fields.push("frames");
        }
        Some(2) => {
            fields.push("operation_id");
            if value["operation_id"] != "clone.seed-runtime.departments-plan" {
                return Err("原部门计划失败的 Rust 操作标识不匹配".into());
            }
        }
        _ => return Err("原部门计划失败收据版本不受支持".into()),
    }
    document::exact(&value, &fields, "部门失败收据")?;
    if value["kind"] != "devex-stage-failure"
        || value["attempt"].as_u64() != Some(number)
        || value["stage"] != "seed-runtime"
        || value["mode"] != "departments-plan"
        || value["error_type"] != attempt["error_type"]
        || value["controller"] != *controller
    {
        return Err("原部门计划失败收据不完整或不属于候选 attempt".into());
    }
    if value["format_version"] == 1 {
        let frames = document::array(&value["frames"], "部门失败栈")?;
        for frame in frames {
            document::exact(frame, &["file", "function", "line"], "部门失败栈帧")?;
            if !document::text(&frame["file"], "部门失败源文件")?.starts_with("tools/python/")
                || document::text(&frame["function"], "部门失败函数")?.is_empty()
                || frame["line"].as_u64().filter(|n| *n > 0).is_none()
            {
                return Err("原部门计划失败栈结构无效".into());
            }
        }
        if !frames
            .iter()
            .any(|f| f["file"] == "tools/python/devex_clone_run.py" && f["function"] == "execute")
        {
            return Err("原部门计划失败未绑定统一阶段执行边界".into());
        }
    }
    if common::read_bound(backend, controller, "部门失败控制器")? != *controller_value {
        return Err("部门计划失败收据与控制器不同".into());
    }
    common::bound_file(backend, &descriptor, "部门失败收据")?;
    Ok((descriptor, value))
}

pub(super) fn prefix(value: &Value, expected: &[(&str, &str)]) -> bool {
    value["format_version"] == 1
        && value["frames"].as_array().is_some_and(|frames| {
            frames.len() >= expected.len()
                && frames
                    .iter()
                    .zip(expected)
                    .all(|(f, (file, function))| f["file"] == *file && f["function"] == *function)
        })
}
