//! 排空快照与运行状态的闭集模型。
use crate::{Result, recovery::clone::native::json as document};
use regex::Regex;
use serde_json::{Value, json};
use std::path::Path;

pub(super) const ACTIVE: &[&str] = &[
    "background_job_status",
    "background_job_lease",
    "background_job_completion",
    "background_job_attempt",
    "outbox_event_status",
    "outbox_event_lease",
    "export_job_status",
    "export_job_request",
    "export_job_delete",
    "user_import_job",
    "enabled_schedule",
    "schedule_execution",
    "config_bundle",
    "config_transfer",
    "retention_run",
    "password_reset",
    "tenant_operation_lease",
    "tenant_data_migration",
    "tenant_data_migration_item",
    "file_upload",
    "tenant_status",
];
pub(super) const TERMINAL: &[&str] = &[
    "background_job_dead",
    "background_job_attempt_failed",
    "background_job_attempt_dead",
    "background_job_attempt_deferred",
    "background_job_attempt_lease_expired",
    "outbox_event_dead",
    "export_job_failed",
    "export_job_cancelled",
    "export_job_expired",
    "user_import_partial",
    "user_import_failed",
    "user_import_cancelled",
    "schedule_skipped_misfire",
    "schedule_skipped_concurrency",
    "schedule_target_unavailable",
    "schedule_invalid_configuration",
    "config_bundle_failed",
    "config_bundle_expired",
    "config_transfer_failed",
    "retention_run_failed",
    "password_reset_expired",
];

pub(super) fn snapshot(value: &Value) -> Result<()> {
    document::exact(
        value,
        &["database_time", "active", "terminal_non_success"],
        "排空快照",
    )?;
    let time = document::text(&value["database_time"], "排空数据库时间")?;
    if !Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{6}Z$")?
        .is_match(time)
    {
        return Err("排空快照数据库时间无效".into());
    }
    for (field, expected) in [("active", ACTIVE), ("terminal_non_success", TERMINAL)] {
        let counts = document::exact(&value[field], expected, "排空计数")?;
        if counts.values().any(|item| item.as_u64().is_none()) {
            return Err("排空快照计数必须是非负整数".into());
        }
    }
    Ok(())
}

pub(super) fn drained(value: &Value) -> Result<bool> {
    snapshot(value)?;
    Ok(value["active"]
        .as_object()
        .ok_or("排空计数不是对象")?
        .values()
        .all(|item| item.as_u64() == Some(0)))
}

pub(super) fn identity(value: &Value) -> Result<()> {
    document::exact(value, &["pid", "started", "executable"], "关闭进程身份")?;
    let started = document::text(&value["started"], "进程创建时间")?;
    if value["pid"].as_u64().is_none_or(|pid| pid <= 1)
        || started.is_empty()
        || !started.bytes().all(|byte| byte.is_ascii_digit())
        || document::text(&value["executable"], "进程可执行文件")?.is_empty()
    {
        return Err("seed 运行进程身份无效".into());
    }
    Ok(())
}

pub(super) fn stopped() -> Value {
    json!({"state":"stopped", "identity":null, "ready":false})
}

pub(super) fn control(
    value: &Value,
    operation: &str,
    roles: &[&str],
    runtime: &Path,
    handoff: &Value,
) -> Result<()> {
    document::exact(
        value,
        &[
            "format_version",
            "kind",
            "operation",
            "scope_id",
            "runtime_directory",
            "processes",
            "producer_history",
        ],
        "关闭运行控制结果",
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "devex-clone-runtime-control"
        || value["operation"] != operation
        || handoff["scope_id"].as_str().is_none()
        || value["scope_id"] != handoff["scope_id"]
        || value["runtime_directory"].as_str() != runtime.to_str()
        || value["producer_history"].as_str() != runtime.join("producer-history.json").to_str()
    {
        return Err("运行控制结果不属于固定 seed handoff".into());
    }
    let processes = document::exact(&value["processes"], roles, "关闭运行进程")?;
    for process in processes.values() {
        document::exact(process, &["state", "identity", "ready"], "关闭进程状态")?;
        document::boolean(&process["ready"], "进程就绪状态")?;
        match process["state"].as_str() {
            Some("running") => identity(&process["identity"])?,
            Some("stopped") if *process == stopped() => (),
            _ => return Err("已停止 seed 进程仍包含身份或就绪状态".into()),
        }
    }
    Ok(())
}

pub(super) fn require(
    value: &Value,
    kind: &str,
    runtime: &Path,
    handoff: &Value,
    identities: &Value,
) -> Result<()> {
    let (operation, roles): (&str, &[&str]) = match kind {
        "api-stopped" => ("stop", &["api"]),
        "worker-stopped" => ("stop", &["worker"]),
        _ => ("status", &["api", "worker"]),
    };
    control(value, operation, roles, runtime, handoff)?;
    let processes = &value["processes"];
    for role in roles {
        let process = &processes[role];
        match kind {
            "api-stopped" | "worker-stopped" | "closed" => {
                if *process != stopped() {
                    return Err("seed 终检要求固定进程精确停止".into());
                }
            }
            "draining" if *role == "api" => {
                if *process != stopped() {
                    return Err("排空期间 API 必须停止".into());
                }
            }
            "observed" if process["state"] == "stopped" => (),
            "initial" | "fixed" | "draining" | "observed" => {
                if process["state"] != "running"
                    || process["ready"].as_bool() != Some(true)
                    || kind != "initial" && process["identity"] != identities[role]
                {
                    return Err("运行中的进程不属于固定就绪代次".into());
                }
            }
            _ => return Err("未知关闭运行状态要求".into()),
        }
    }
    Ok(())
}

pub(super) fn control_request(input: &Value) -> Result<Value> {
    document::exact(
        input,
        &[
            "value",
            "requirement",
            "operation",
            "roles",
            "runtime",
            "handoff",
            "identities",
        ],
        "关闭运行参数",
    )?;
    let runtime = Path::new(document::text(&input["runtime"], "关闭运行目录")?);
    let requirement = document::text(&input["requirement"], "关闭状态要求")?;
    if requirement == "validate" {
        let roles = document::array(&input["roles"], "关闭角色")?
            .iter()
            .map(|role| document::text(role, "关闭角色"))
            .collect::<Result<Vec<_>>>()?;
        if roles.is_empty()
            || roles.len() > 2
            || roles.iter().any(|role| !["api", "worker"].contains(role))
            || roles.len() == 2 && roles[0] == roles[1]
        {
            return Err("关闭角色必须是 API/Worker 的非重复子集".into());
        }
        control(
            &input["value"],
            document::text(&input["operation"], "关闭操作")?,
            &roles,
            runtime,
            &input["handoff"],
        )?;
    } else {
        require(
            &input["value"],
            requirement,
            runtime,
            &input["handoff"],
            &input["identities"],
        )?;
    }
    Ok(input["value"].clone())
}
