//! 关闭阶段结果及稳定排空证据；校验不代表授权重放任何阶段。
use super::close;
use crate::{Result, recovery::clone::native::json as document};
use regex::Regex;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::path::Path;

const FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "status",
    "phase",
    "handoff",
    "runtime_directory",
    "scope_id",
    "process_identities",
    "runtime_status",
    "api_stop",
    "drain",
    "worker_stop",
    "final_snapshot",
    "last_snapshot",
    "terminal_non_success",
    "stopped_evidence",
    "worker_preserved",
    "reason",
    "error_type",
    "outbox_drained",
    "direct_remote_writes",
    "worker_drain_writes",
    "restore_qualified",
];
const REASONS: &[&str] = &[
    "api_stop_failed",
    "post_api_stop_observation_failed",
    "drain_timeout",
    "drain_observation_failed",
    "guard_failed",
    "worker_stop_failed",
    "worker_stop_result_unavailable",
    "final_verification_failed",
    "final_state_not_drained",
];

pub(super) fn drain(value: &Value, worker: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "stable_seconds",
            "observations",
            "first_snapshot",
            "last_snapshot",
        ],
        "稳定排空证据",
    )?;
    if value["stable_seconds"].as_u64() != Some(2)
        || value["observations"].as_u64().is_none_or(|count| count < 2)
    {
        return Err("稳定排空证据参数无效".into());
    }
    let first = &value["first_snapshot"];
    let last = &value["last_snapshot"];
    if !close::drained(first)?
        || !close::drained(last)?
        || first["active"] != last["active"]
        || first["terminal_non_success"] != last["terminal_non_success"]
    {
        return Err("稳定排空证据仍有活跃状态或前后变化".into());
    }
    close::identity(worker)
}

pub(super) fn validate(
    value: &Value,
    runtime: &Path,
    handoff: &Value,
    binding: &Value,
    in_progress: bool,
) -> Result<()> {
    document::exact(value, FIELDS, "关闭阶段结果")?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-seed-close"
        || value["handoff"] != *binding
        || value["runtime_directory"].as_str() != runtime.to_str()
        || value["scope_id"] != handoff["scope_id"]
        || value["direct_remote_writes"].as_u64() != Some(0)
        || value["worker_drain_writes"].as_bool() != Some(true)
        || value["restore_qualified"].as_bool() != Some(false)
    {
        return Err("历史 close 结果不属于固定 handoff".into());
    }
    let identities = &value["process_identities"];
    document::exact(identities, &["api", "worker"], "关闭代次身份")?;
    for role in ["api", "worker"] {
        close::identity(&identities[role])?;
    }
    close::require(
        &value["runtime_status"],
        "observed",
        runtime,
        handoff,
        identities,
    )?;
    let phase = document::text(&value["phase"], "关闭阶段")?;
    if ![
        "running",
        "api_stopped",
        "drained",
        "worker_stopped",
        "verified",
    ]
    .contains(&phase)
    {
        return Err("历史 close 阶段无效".into());
    }
    stage(value, phase, runtime, handoff, identities)?;
    for key in ["final_snapshot", "last_snapshot"] {
        if !value[key].is_null() {
            close::snapshot(&value[key])?;
        }
    }
    let source = if value["final_snapshot"].is_null() {
        &value["last_snapshot"]
    } else {
        &value["final_snapshot"]
    };
    if value["terminal_non_success"] != source["terminal_non_success"]
        || value["worker_preserved"].as_bool()
            != Some(["running", "api_stopped", "drained"].contains(&phase))
    {
        return Err("历史 close 最后快照或 Worker 保留语义无效".into());
    }
    status(value, phase, in_progress)
}

fn stage(
    value: &Value,
    phase: &str,
    runtime: &Path,
    handoff: &Value,
    identities: &Value,
) -> Result<()> {
    if phase == "running" {
        close::require(
            &value["runtime_status"],
            "fixed",
            runtime,
            handoff,
            identities,
        )?;
        if !value["api_stop"].is_null() {
            return Err("运行阶段不能包含 API 停止结果".into());
        }
    } else if value["api_stop"].is_null() {
        close::require(
            &value["runtime_status"],
            "draining",
            runtime,
            handoff,
            identities,
        )?;
    } else {
        close::require(
            &value["api_stop"],
            "api-stopped",
            runtime,
            handoff,
            &Value::Null,
        )?;
    }
    if ["running", "api_stopped"].contains(&phase) {
        if ["drain", "worker_stop", "final_snapshot"]
            .iter()
            .any(|key| !value[key].is_null())
        {
            return Err("close 当前阶段包含未来阶段证据".into());
        }
    } else {
        drain(&value["drain"], &identities["worker"])?;
    }
    if ["running", "api_stopped", "drained"].contains(&phase) && !value["worker_stop"].is_null() {
        return Err("Worker 尚未停止却包含停止结果".into());
    }
    if !value["worker_stop"].is_null() {
        close::require(
            &value["worker_stop"],
            "worker-stopped",
            runtime,
            handoff,
            &Value::Null,
        )?;
    }
    Ok(())
}

fn status(value: &Value, phase: &str, in_progress: bool) -> Result<()> {
    if value["status"] == "seed_runtime_closed" {
        if phase != "verified"
            || !value["reason"].is_null()
            || !value["error_type"].is_null()
            || value["outbox_drained"].as_bool() != Some(true)
            || value["final_snapshot"].is_null()
            || !close::drained(&value["final_snapshot"])?
            || value["stopped_evidence"].is_null()
        {
            return Err("历史 close 成功结果不完整".into());
        }
    } else if value["status"] != "needs_reconciliation"
        || phase == "verified"
        || value["reason"].is_null() && (!in_progress || !value["error_type"].is_null())
        || !value["reason"].is_null()
            && !value["reason"]
                .as_str()
                .is_some_and(|reason| REASONS.contains(&reason))
        || value["outbox_drained"].as_bool() != Some(false)
        || !value["error_type"].is_null()
            && !value["error_type"].as_str().is_some_and(|error| {
                Regex::new(r"^[A-Za-z_][A-Za-z0-9_.]*$")
                    .is_ok_and(|pattern| pattern.is_match(error))
            })
    {
        return Err("历史 close 待核对结果无效".into());
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn fixture() -> (Value, Value, Value) {
    let runtime = Path::new("fixture");
    let handoff = json!({"scope_id":"seed-fixture"});
    let identity = json!({"pid":101,"started":"123","executable":"api.exe"});
    let control = json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"status",
        "scope_id":"seed-fixture","runtime_directory":runtime,"producer_history":runtime.join("producer-history.json"),
        "processes":{"api":{"state":"running","ready":true,"identity":identity},
            "worker":{"state":"running","ready":true,"identity":identity}}});
    let value = json!({"format_version":1,"kind":"devex-clone-seed-close","status":"needs_reconciliation",
        "phase":"running","handoff":{},"runtime_directory":runtime,"scope_id":"seed-fixture",
        "process_identities":{"api":identity,"worker":identity},"runtime_status":control,
        "api_stop":null,"drain":null,"worker_stop":null,"final_snapshot":null,"last_snapshot":null,
        "terminal_non_success":null,"stopped_evidence":null,"worker_preserved":true,"reason":null,
        "error_type":null,"outbox_drained":false,"direct_remote_writes":0,"worker_drain_writes":true,"restore_qualified":false});
    (value, handoff, identity)
}
