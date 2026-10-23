use super::{documents as doc, reject};
use crate::{Result, recovery::clone::native::json as strict};
use serde_json::Value;

pub(super) fn execute(args: &Value) -> Result<Value> {
    strict::exact(
        args,
        &[
            "mode",
            "directory",
            "source",
            "owner",
            "request",
            "value",
            "candidate",
            "supervisor",
            "runtime_sha256",
            "backend",
            "runtime",
        ],
        "Worker 收据核验",
    )?;
    let owner = &args["owner"];
    let request = &args["request"];
    operation(
        owner,
        request,
        &args["directory"],
        &args["source"],
        &args["runtime_sha256"],
    )?;
    match args["mode"].as_str() {
        Some("operation") => Ok(Value::Null),
        Some("request") => {
            runtime(request, &args["backend"], &args["runtime"])?;
            Ok(Value::Null)
        }
        Some("candidate") => child(&args["value"], owner, request, None),
        Some("supervisor") => child(&args["value"], owner, request, Some(&args["candidate"])),
        Some("progress") => progress(
            &args["value"],
            owner,
            request,
            &args["candidate"],
            &args["supervisor"],
        ),
        Some("result") => result(&args["value"], owner, request),
        _ => reject("未知 Worker 收据核验操作"),
    }
}

fn operation(
    owner: &Value,
    request: &Value,
    directory: &Value,
    source: &Value,
    runtime_sha: &Value,
) -> Result<()> {
    doc::common(owner, "full-stack-worker-control-owner", directory, source)?;
    doc::common(
        request,
        "full-stack-worker-control-request",
        directory,
        source,
    )?;
    doc::fields(
        owner,
        &["identity", "created_at_ns"],
        "Worker owner 字段无效",
    )?;
    doc::identity(&owner["identity"])?;
    doc::fields(
        request,
        &[
            "operation",
            "backend_root",
            "scope_id",
            "controller_identity",
            "owner_sha256",
            "runtime_receipt_sha256",
            "worker_artifact",
            "worker_ready_url",
            "timeout_seconds",
            "log",
            "requested_at_ns",
        ],
        "Worker 控制请求字段无效",
    )?;
    let operation = request["operation"].as_str().unwrap_or_default();
    let expected_log = if operation == "start" {
        Value::String(format!(
            "worker-{}.log",
            owner["operation_id"].as_str().unwrap_or_default()
        ))
    } else {
        Value::Null
    };
    if !doc::positive(&owner["created_at_ns"])
        || !doc::positive(&request["requested_at_ns"])
        || !matches!(
            operation,
            "start" | "stop" | "crash" | "status" | "reconcile"
        )
        || request["operation_id"] != owner["operation_id"]
        || request["owner_sha256"] != doc::hash(owner)?
        || request["controller_identity"] != owner["identity"]
        || request["log"] != expected_log
        || !doc::hex(runtime_sha, 64)
        || request["runtime_receipt_sha256"] != *runtime_sha
    {
        return reject("Worker 控制请求没有绑定原子 owner 或当前 runtime 收据");
    }
    Ok(())
}

fn runtime(request: &Value, backend: &Value, runtime: &Value) -> Result<()> {
    if request["backend_root"] != *backend
        || request["scope_id"] != runtime["scope_id"]
        || request["worker_artifact"] != runtime["artifacts"]["worker"]
        || request["worker_ready_url"] != runtime["worker_ready_url"]
        || !request["scope_id"].as_str().is_some_and(|s| !s.is_empty())
        || !request["timeout_seconds"]
            .as_f64()
            .is_some_and(|n| n.is_finite() && n > 0.0 && n <= 180.0)
    {
        return reject("Worker 控制请求与当前后端、scope、产物或端点不匹配");
    }
    Ok(())
}

fn receipt_common(value: &Value, kind: &str, owner: &Value, request: &Value) -> Result<()> {
    doc::common(
        value,
        kind,
        &owner["runtime_directory"],
        &owner["source_sha256"],
    )?;
    doc::bound(value, owner, request)
}

fn child(
    value: &Value,
    owner: &Value,
    request: &Value,
    candidate: Option<&Value>,
) -> Result<Value> {
    let (kind, extras, timestamp) = if candidate.is_some() {
        (
            "full-stack-worker-control-supervisor",
            &[
                "identity",
                "owner_sha256",
                "request_sha256",
                "candidate_sha256",
                "authorized_at_ns",
            ][..],
            "authorized_at_ns",
        )
    } else {
        (
            "full-stack-worker-control-candidate",
            &[
                "identity",
                "owner_sha256",
                "request_sha256",
                "created_at_ns",
            ][..],
            "created_at_ns",
        )
    };
    receipt_common(value, kind, owner, request)?;
    doc::fields(value, extras, "Worker 启动监督收据字段无效")?;
    doc::identity(&value["identity"])?;
    if !doc::positive(&value[timestamp]) {
        return reject("Worker 启动监督收据时间戳无效");
    }
    if let Some(candidate) = candidate {
        child(candidate, owner, request, None)?;
        if value["candidate_sha256"] != doc::hash(candidate)?
            || value["identity"] != candidate["identity"]
        {
            return reject("Worker 启动监督授权没有绑定当前候选进程");
        }
    }
    Ok(value.clone())
}

fn progress(
    value: &Value,
    owner: &Value,
    request: &Value,
    candidate: &Value,
    supervisor: &Value,
) -> Result<Value> {
    receipt_common(value, "full-stack-worker-control-progress", owner, request)?;
    doc::fields(
        value,
        &[
            "phase",
            "identity",
            "owner_sha256",
            "request_sha256",
            "supervisor_sha256",
            "updated_at_ns",
        ],
        "Worker 启动进度字段无效",
    )?;
    child(supervisor, owner, request, Some(candidate))?;
    let phase = value["phase"].as_str().unwrap_or_default();
    if !matches!(phase, "launching" | "started" | "ready")
        || (phase == "launching" && !value["identity"].is_null())
        || value["supervisor_sha256"] != doc::hash(supervisor)?
        || !doc::positive(&value["updated_at_ns"])
    {
        return reject("Worker 启动进度没有绑定当前控制操作");
    }
    if phase != "launching" {
        doc::identity(&value["identity"])?;
    }
    Ok(value.clone())
}

fn result(value: &Value, owner: &Value, request: &Value) -> Result<Value> {
    receipt_common(value, "full-stack-worker-control-result", owner, request)?;
    doc::fields(
        value,
        &[
            "operation",
            "scope_id",
            "controller_identity",
            "owner_sha256",
            "request_sha256",
            "outcome",
            "state",
            "identity",
            "log",
            "reconciled",
            "error_type",
            "error",
            "completed_at_ns",
        ],
        "Worker 控制结果字段无效",
    )?;
    let state = value["state"].as_str().unwrap_or_default();
    let outcome = value["outcome"].as_str().unwrap_or_default();
    if value["operation"] != request["operation"]
        || value["scope_id"] != request["scope_id"]
        || value["controller_identity"] != owner["identity"]
        || value["log"] != request["log"]
        || !matches!(outcome, "succeeded" | "failed")
        || !matches!(state, "running" | "stopped" | "unknown")
        || !value["reconciled"].is_boolean()
        || !doc::positive(&value["completed_at_ns"])
    {
        return reject("Worker 控制结果与当前请求不匹配");
    }
    if !value["identity"].is_null() {
        doc::identity(&value["identity"])?;
    }
    if outcome == "succeeded" {
        if !value["error_type"].is_null() || !value["error"].is_null() || state == "unknown" {
            return reject("成功的 Worker 控制结果包含错误或未知状态");
        }
        if matches!(request["operation"].as_str(), Some("start")) && state != "running"
            || matches!(request["operation"].as_str(), Some("stop" | "crash")) && state != "stopped"
        {
            return reject("Worker 控制结果没有达到请求的目标状态");
        }
    } else if !matches!(state, "stopped" | "unknown")
        || !value["error_type"].as_str().is_some_and(|s| !s.is_empty())
        || !value["error"].as_str().is_some_and(|s| !s.is_empty())
    {
        return reject("失败的 Worker 控制结果缺少错误信息或包含错误状态");
    }
    if state == "running" && value["identity"].is_null()
        || state == "stopped" && !value["identity"].is_null()
    {
        return reject("Worker 控制结果状态与进程身份不一致");
    }
    Ok(value.clone())
}
