use std::collections::{BTreeMap, BTreeSet};

use super::{
    input::{ArtifactBinding, Database},
    physical,
    schedule::{TRIGGERS, normalize_decimal},
    sql::{Row, SqlValue},
};
use crate::Result;
use chrono::NaiveDateTime;
use regex::Regex;
use serde_json::Value;

const EMPTY: &[&str] = &[
    "sys_tenant_operation_lease",
    "sys_tenant_data_migration",
    "sys_tenant_data_migration_item",
];

const CLEANUP_JOBS: &[(&str, &str)] = &[
    ("system.export_result_cleanup", "system.export.cleanup"),
    (
        "system.message_retention_cleanup",
        "system.message.retention",
    ),
    (
        "system.data_retention_cleanup",
        "system.data.retention.cleanup",
    ),
];

type Triple = (String, String, String);
type Slot = (Option<String>, String, Option<String>);

#[derive(Default)]
pub(crate) struct Observation {
    tenants: BTreeSet<String>,
    placements: BTreeMap<String, Triple>,
    fences: BTreeMap<String, Triple>,
    slots: BTreeMap<String, Slot>,
    jobs: BTreeMap<String, Row>,
    attempts: BTreeSet<String>,
    schedules: BTreeMap<String, Row>,
    executions: Vec<Row>,
    files: Vec<(String, String, i64, String)>,
}

pub(crate) fn collect(state: &mut Observation, table: &str, row: &Row, key: &str) -> Result<()> {
    match table {
        "sys_tenant" => {
            state
                .tenants
                .insert(required_text(row, "tenant_id")?.to_owned());
        }
        "sys_tenant_data_placement" | "biz_tenant_fence" => {
            let field = if table == "sys_tenant_data_placement" {
                "current_target_key"
            } else {
                "target_key"
            };
            let tenant = required_text(row, "tenant_id")?.to_owned();
            let target = required_text(row, field)?.to_owned();
            let generation = required_integer(row, "placement_generation")?;
            let token = required_text(row, "switch_token")?;
            if required_text(row, "state")? != "active" || generation <= 0 || token.is_empty() {
                return Err("租户 placement/fence 非 active、重复或缺少代次令牌".into());
            }
            if table == "biz_tenant_fence" && target != key {
                return Err("fence 与当前物理逻辑目标不一致".into());
            }
            let group = if table == "sys_tenant_data_placement" {
                &mut state.placements
            } else {
                &mut state.fences
            };
            if group
                .insert(tenant, (target, generation.to_string(), token.to_owned()))
                .is_some()
            {
                return Err("租户 placement/fence 非 active、重复或缺少代次令牌".into());
            }
        }
        "biz_tenant_target_slot" => {
            if required_integer(row, "slot_id")? != 1 || state.slots.contains_key(key) {
                return Err("独立目标 slot 重复或不是固定槽位".into());
            }
            state.slots.insert(
                key.to_owned(),
                (
                    optional_text(row, "tenant_id")?,
                    optional_number_text(row, "placement_generation")?
                        .unwrap_or_else(|| "None".into()),
                    optional_text(row, "switch_token")?,
                ),
            );
        }
        "sys_background_job" => {
            let id = required_identifier(row, "id")?;
            if state.jobs.insert(id, row.clone()).is_some() {
                return Err("完整后台任务 ID 重复".into());
            }
        }
        "sys_background_job_attempt" => {
            state.attempts.insert(required_identifier(row, "job_id")?);
        }
        "sys_job_schedule_execution" => {
            if !row.get("background_job_id").is_none_or(SqlValue::is_null) {
                state.executions.push(row.clone());
            }
        }
        "sys_job_schedule" => {
            let id = required_identifier(row, "id")?;
            if state.schedules.insert(id, row.clone()).is_some() {
                return Err("完整调度计划 ID 重复".into());
            }
        }
        "sys_file" if required_text(row, "del_flag")? == "0" => {
            state.files.push((
                required_text(row, "bucket")?.to_owned(),
                required_text(row, "storage_path")?.to_owned(),
                required_integer(row, "file_size")?,
                required_text(row, "file_sha256")?.to_owned(),
            ));
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn validate_state(table: &str, row: &Row) -> Result<()> {
    if EMPTY.contains(&table) {
        return Err("租户租约或迁移历史需单独审查，不能自动取消、删行或复制".into());
    }
    let allowed_status: Option<&[&str]> = match table {
        "sys_background_job" => Some(&["succeeded", "dead"]),
        "sys_outbox_event" => Some(&["published", "dead"]),
        "sys_export_job" => Some(&["succeeded", "failed", "cancelled", "expired"]),
        "sys_user_import_job" => Some(&["succeeded", "partial", "failed", "cancelled"]),
        "sys_tenant_config_bundle" => Some(&["succeeded", "failed", "expired"]),
        "sys_tenant_config_transfer" => Some(&[
            "preview_ready",
            "previewed",
            "applied",
            "rolled_back",
            "failed",
        ]),
        "sys_data_retention_run" => Some(&["succeeded", "failed"]),
        "password_reset_requests" => Some(&["completed", "expired"]),
        _ => None,
    };
    if allowed_status.is_some_and(|allowed| {
        row.get("status")
            .and_then(SqlValue::text)
            .is_none_or(|value| !allowed.contains(&value))
    }) {
        return Err("来源存在未解决的可执行任务或工作流".into());
    }
    if matches!(table, "sys_background_job" | "sys_outbox_event")
        && ["lease_owner", "lease_until"]
            .iter()
            .any(|name| row.get(*name).is_none_or(|value| !value.is_null()))
    {
        return Err("终态任务仍携带租约".into());
    }
    if table == "sys_background_job" && null(row, "completed_at") {
        return Err("终态任务缺少完成时间".into());
    }
    if table == "sys_background_job_attempt" {
        validate_attempt(row)?;
    }
    if table == "sys_job_schedule_execution" {
        validate_execution(row)?;
    }
    if table == "sys_job_schedule"
        && required_text(row, "del_flag")? != "2"
        && required_integer(row, "enabled")? != 0
    {
        return Err("来源仍有可触发调度计划".into());
    }
    if table == "sys_export_job"
        && ["active_request_fingerprint", "delete_pending_at"]
            .iter()
            .any(|name| row.get(*name).is_none_or(|value| !value.is_null()))
    {
        return Err("导出仍有活跃请求或待删除状态".into());
    }
    if table == "sys_file" && required_text(row, "upload_status")? != "ready" {
        return Err("来源文件仍在上传或待清理".into());
    }
    if table == "sys_tenant" {
        let status = required_text(row, "status")?;
        if !matches!(status, "enabled" | "disabled") {
            return Err("来源租户仍在开通或失败维护状态".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_relations(
    state: &Observation,
    databases: &BTreeMap<String, Database>,
    objects: &BTreeMap<(String, String), ArtifactBinding>,
) -> Result<()> {
    let tenants = state.tenants.iter().cloned().collect::<BTreeSet<_>>();
    if tenants != state.placements.keys().cloned().collect::<BTreeSet<_>>()
        || state.placements != state.fences
    {
        return Err("租户、placement 与 fence 必须完整且代次令牌一致".into());
    }
    for (target, _, _) in state.placements.values() {
        if !databases.contains_key(target) {
            return Err("placement 引用未登记的目标".into());
        }
    }
    for (key, database) in databases {
        let residents = state
            .placements
            .iter()
            .filter(|(_, (target, _, _))| target == key)
            .map(|(tenant, value)| (Some(tenant.clone()), value.1.clone(), Some(value.2.clone())))
            .collect::<Vec<_>>();
        let slot = state.slots.get(key).cloned().unwrap_or_else(empty_slot);
        if database.mode == "dedicated" {
            if residents.len() > 1 || slot != residents.first().cloned().unwrap_or_else(empty_slot)
            {
                return Err("dedicated slot 与唯一驻留租户不一致".into());
            }
        } else if slot != empty_slot() {
            return Err("共享目标必须保留当前基线初始化的空 slot".into());
        }
    }
    validate_job_relations(state)?;
    for (bucket, key, size, sha) in &state.files {
        let artifact = objects
            .get(&(bucket.clone(), key.clone()))
            .ok_or("有效文件的逻辑 key、字节数或内容摘要与对象清单不一致")?;
        if artifact.digest.bytes != *size as u64 || artifact.digest.sha256 != *sha {
            return Err("有效文件的逻辑 key、字节数或内容摘要与对象清单不一致".into());
        }
    }
    Ok(())
}

pub(crate) fn reject_physical(
    row: &Row,
    forbidden: &[String],
    logical: &BTreeSet<String>,
) -> Result<()> {
    physical::reject_row(row, forbidden, logical)
}

fn validate_attempt(row: &Row) -> Result<()> {
    let outcome = required_text(row, "outcome")?;
    if !matches!(
        outcome,
        "succeeded" | "failed" | "dead" | "deferred" | "lease_expired"
    ) || null(row, "closed_at")
    {
        return Err("后台任务尝试尚未闭合".into());
    }
    if (outcome == "lease_expired") != null(row, "finished_at") {
        return Err("后台任务尝试完成时间语义无效".into());
    }
    Ok(())
}

fn validate_execution(row: &Row) -> Result<()> {
    let outcome = required_text(row, "outcome")?;
    if !matches!(
        outcome,
        "enqueued"
            | "skipped_misfire"
            | "skipped_concurrency"
            | "target_unavailable"
            | "invalid_configuration"
    ) {
        return Err("未知调度执行结果".into());
    }
    if (outcome == "enqueued")
        != row
            .get("background_job_id")
            .is_some_and(|value| !value.is_null())
    {
        return Err("调度触发历史与后台任务关联不一致".into());
    }
    Ok(())
}

fn validate_job_relations(state: &Observation) -> Result<()> {
    if !state
        .attempts
        .is_subset(&state.jobs.keys().cloned().collect())
    {
        return Err("任务尝试引用不存在的完整任务".into());
    }
    for execution in &state.executions {
        let job = state
            .jobs
            .get(&required_identifier(execution, "background_job_id")?)
            .ok_or("调度历史缺少父任务证据；可能已被保留策略清除，不能推断已闭合")?;
        let schedule = state
            .schedules
            .get(&required_identifier(execution, "schedule_id")?);
        let cleanup_type = job_type_is_cleanup(job)?;
        let cleanup_schedule = schedule.is_some_and(|row| {
            row.get("handler_key")
                .and_then(SqlValue::text)
                .is_some_and(is_cleanup_handler)
        });
        if null(job, "tenant_id") || cleanup_type || cleanup_schedule {
            validate_system_cleanup(job, execution, schedule)?;
        } else if job.get("tenant_id").and_then(SqlValue::text)
            != execution.get("tenant_id").and_then(SqlValue::text)
        {
            return Err("调度执行与终态父任务不属于同一租户".into());
        }
    }
    Ok(())
}

fn validate_system_cleanup(job: &Row, execution: &Row, schedule: Option<&Row>) -> Result<()> {
    let schedule = schedule.ok_or("全局调度父任务仅允许精确 system 维护目标及其登记 job type")?;
    let handler = required_text(schedule, "handler_key")?;
    let expected_type = CLEANUP_JOBS
        .iter()
        .find_map(|(handler_key, job_type)| (*handler_key == handler).then_some(*job_type))
        .ok_or("全局调度父任务仅允许精确 system 维护目标及其登记 job type")?;
    if required_text(schedule, "tenant_id")? != "system"
        || required_text(execution, "tenant_id")? != "system"
        || !null(job, "tenant_id")
        || required_text(job, "job_type")? != expected_type
    {
        return Err("全局调度父任务仅允许精确 system 维护目标及其登记 job type".into());
    }
    let schedule_id = positive_id(schedule.get("id").ok_or("调度计划缺少 id")?)?;
    if positive_id(job.get("schedule_id").ok_or("后台任务缺少 schedule_id")?)? != schedule_id
        || positive_id(
            execution
                .get("schedule_id")
                .ok_or("调度执行缺少 schedule_id")?,
        )? != schedule_id
        || positive_id(job.get("id").ok_or("后台任务缺少 id")?)?
            != positive_id(
                execution
                    .get("background_job_id")
                    .ok_or("调度执行缺少 background_job_id")?,
            )?
    {
        return Err("平台维护 schedule、execution 与 parent ID 不一致".into());
    }
    validate_state("sys_background_job", job)?;
    validate_state("sys_job_schedule_execution", execution)?;
    let scheduled = timing(execution.get("scheduled_for"), false)?;
    if timing(job.get("scheduled_for"), false)? != scheduled
        || timing(job.get("completed_at"), false)? < scheduled
    {
        return Err("平台维护父任务的计划时间或完成时间不匹配".into());
    }
    let payload = job
        .get("payload")
        .and_then(|value| match value {
            SqlValue::Json(Value::Object(value)) => Some(value),
            _ => None,
        })
        .ok_or("平台维护父任务必须保留当前受控载荷及精确触发时间")?;
    let expected = ["schedule_id", "scheduled_for", "trigger_kind"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let trigger = required_text(execution, "trigger_kind")?;
    if payload.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected
        || payload.get("schedule_id").and_then(Value::as_str) != Some(schedule_id.as_str())
        || !TRIGGERS.contains(&trigger)
        || payload.get("trigger_kind").and_then(Value::as_str) != Some(trigger)
        || timing_json(payload.get("scheduled_for"))? != scheduled
    {
        return Err("平台维护父任务必须保留当前受控载荷及精确触发时间".into());
    }
    Ok(())
}

fn timing(value: Option<&SqlValue>, payload: bool) -> Result<NaiveDateTime> {
    let value = value
        .and_then(SqlValue::text)
        .ok_or("平台维护关联时间必须是完整 UTC 微秒时间，不截断未知精度")?;
    parse_timing(value, payload)
}

fn timing_json(value: Option<&Value>) -> Result<NaiveDateTime> {
    let value = value
        .and_then(Value::as_str)
        .ok_or("平台维护关联时间必须是完整 UTC 微秒时间，不截断未知精度")?;
    parse_timing(value, true)
}

fn parse_timing(value: &str, payload: bool) -> Result<NaiveDateTime> {
    let pattern = if payload {
        r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?Z$"
    } else {
        r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d{1,6})?$"
    };
    if !Regex::new(pattern)?.is_match(value) {
        return Err("平台维护关联时间必须是完整 UTC 微秒时间，不截断未知精度".into());
    }
    let value = value.strip_suffix('Z').unwrap_or(value).replace('T', " ");
    NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|_| "平台维护关联时间必须是完整 UTC 微秒时间，不截断未知精度".into())
}

fn positive_id(value: &SqlValue) -> Result<String> {
    let text = match value {
        SqlValue::Number(value) => normalize_decimal(value)?,
        _ => return Err("平台维护关联必须使用明确整数 ID".into()),
    };
    let parsed = text
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or("平台维护关联 ID 超出正整数范围")?;
    Ok(parsed.to_string())
}

fn job_type_is_cleanup(job: &Row) -> Result<bool> {
    Ok(job
        .get("job_type")
        .and_then(SqlValue::text)
        .is_some_and(|value| CLEANUP_JOBS.iter().any(|(_, job_type)| *job_type == value)))
}

fn is_cleanup_handler(value: &str) -> bool {
    CLEANUP_JOBS.iter().any(|(handler, _)| *handler == value)
}

fn empty_slot() -> Slot {
    (None, "None".into(), None)
}

fn optional_text(row: &Row, name: &str) -> Result<Option<String>> {
    match row.get(name) {
        None | Some(SqlValue::Null) => Ok(None),
        Some(SqlValue::Text(value)) => Ok(Some(value.clone())),
        _ => Err(format!("复制行 {name} 必须为字符串或 NULL").into()),
    }
}

fn optional_number_text(row: &Row, name: &str) -> Result<Option<String>> {
    match row.get(name) {
        None | Some(SqlValue::Null) => Ok(None),
        Some(SqlValue::Number(value)) => Ok(Some(value.clone())),
        _ => Err(format!("复制行 {name} 必须为数值或 NULL").into()),
    }
}

fn null(row: &Row, name: &str) -> bool {
    row.get(name).is_none_or(SqlValue::is_null)
}

fn required_text<'a>(row: &'a Row, name: &str) -> Result<&'a str> {
    row.get(name)
        .and_then(SqlValue::text)
        .ok_or_else(|| format!("复制行 {name} 必须为字符串").into())
}

fn required_integer(row: &Row, name: &str) -> Result<i64> {
    row.get(name)
        .and_then(SqlValue::integer)
        .ok_or_else(|| format!("复制行 {name} 必须为整数").into())
}

fn required_identifier(row: &Row, name: &str) -> Result<String> {
    let value = row.get(name).ok_or_else(|| format!("复制行缺少 {name}"))?;
    match value {
        SqlValue::Text(value) => Ok(value.clone()),
        SqlValue::Number(value) => {
            Ok(normalize_decimal(value).map_err(|_| format!("复制行 {name} 必须为整数"))?)
        }
        _ => Err(format!("复制行 {name} 必须为整数标识").into()),
    }
}
