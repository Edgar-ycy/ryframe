//! 只读完整 system 调度行与精确的停用前后像。
mod guard;
pub(in crate::recovery::clone::native) use guard::read_target_control;
pub(super) use guard::{
    require_disabled_schedules, verify_administrator, verify_source_and_target,
    verify_target_databases,
};

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Map, Value, json};

use super::super::{
    catalog::{self, Columns},
    digest,
    export::transport::{CommandSpec, StdoutTarget, Transport},
    json as document,
    resources::common,
    s3::app_configuration,
    schedule,
    sql::{Row, SqlValue},
    target_verify::{
        database::{evidence, model},
        guard::Inputs,
        input::{request_configuration, request_physical::defaults_connection},
        publication,
        storage::StorageInputs,
    },
};
use crate::{
    Result,
    cli::CloneSide,
    utc_timestamp::{UtcOrderKey, mysql_datetime_key, mysql_datetime_micros},
};

const ACTION_FIELDS: &[&str] = &[
    "action",
    "tenant_id",
    "schedule_id",
    "handler_key",
    "expected_version",
    "source_row_sha256",
];
const CHANGED_FIELDS: &[&str] = &["enabled", "next_run_at", "version", "updated_at"];
const SQL_UTC_FORMAT: &str = "%Y-%m-%d %H:%i:%s.%f";

pub(in crate::recovery::clone::native) fn action_input(pending: &Value) -> Result<Value> {
    let fields = document::object(pending, "复制后调度动作")?;
    let action = ACTION_FIELDS
        .iter()
        .map(|&field| {
            let value = fields.get(field).ok_or("调度待办缺少原动作字段")?;
            Ok((field.to_owned(), value.clone()))
        })
        .collect::<Result<Map<_, _>>>()?;
    let action = Value::Object(action);
    schedule::actions(&actions(&action))?;
    Ok(action)
}

pub(in crate::recovery::clone::native) fn validate_api_row(
    value: &Value,
    action: &Value,
    enabled: bool,
    version: i64,
) -> Result<()> {
    let action = action_input(action)?;
    document::object(value, "调度 API 行")?;
    if value["id"] != action["schedule_id"]
        || value["handler_key"] != action["handler_key"]
        || value["enabled"].as_bool() != Some(enabled)
        || value["version"].as_i64() != Some(version)
    {
        return Err("调度 API 行与明确 ID、handler、状态或版本不一致".into());
    }
    Ok(())
}

pub(in crate::recovery::clone::native) fn validate_before(
    execution: &Path,
    action: &Value,
    observed: &Value,
) -> Result<()> {
    let action = action_input(action)?;
    let (_, row) = observation(observed, &columns(execution)?)?;
    if action["source_row_sha256"] != schedule::row_sha256(&row)? {
        return Err("写前完整目标行不等于精确源行摘要".into());
    }
    Ok(())
}

pub(in crate::recovery::clone::native) fn inspect_disabled(
    execution: &Path,
    action: &Value,
    before: &Value,
    current: &Value,
) -> Result<Value> {
    let action = action_input(action)?;
    let columns = columns(execution)?;
    let (started, old) = observation(before, &columns)?;
    let (finished, observed) = observation(current, &columns)?;
    let mut pending = schedule::actions(&actions(&action))?;
    if schedule::inspect(&old, &mut pending)?.is_none() || !pending.is_empty() {
        return Err("停用核对没有完整匹配原启用调度的计划行".into());
    }
    if started > finished {
        return Err("调度变更的数据库时间窗口顺序错误".into());
    }
    let status = if observed == old {
        "unchanged_before"
    } else if disabled(&old, &observed, started, finished)? {
        "disabled_row_verified"
    } else {
        "needs_reconciliation"
    };
    Ok(json!({
        "action_sha256":digest::plan_hash(&action)?,
        "before_sha256":schedule::row_sha256(&old)?,
        "observed_sha256":schedule::row_sha256(&observed)?,
        "schedule_id":action["schedule_id"],"tenant_id":action["tenant_id"],
        "automatic_retry_allowed":false,"target_ready":false,"api_execution_proven":false,
        "status":status,"worker_must_remain_stopped":true,
    }))
}

fn actions(action: &Value) -> Value {
    json!({"copy_stage":"source_to_seed","target_schedule_actions":[action]})
}

fn columns(execution: &Path) -> Result<Columns> {
    catalog::load(execution)?
        .control
        .remove("sys_job_schedule")
        .ok_or_else(|| "控制库 catalog 缺少调度完整列".into())
}

fn observation(value: &Value, columns: &Columns) -> Result<(UtcOrderKey, Row)> {
    document::exact(value, &["database_now", "row"], "目标调度完整读取")?;
    let current = database_time(&value["database_now"])?;
    let fields = document::object(&value["row"], "目标调度完整行")?;
    if fields.len() != columns.len() {
        return Err("必须读取调度当前 schema 的完整数据库行".into());
    }
    let row = columns
        .iter()
        .map(|(name, kind)| {
            let value = fields.get(name).ok_or("调度当前行缺少 catalog 列")?;
            let value = match (kind.as_str(), value) {
                (_, Value::Null) => SqlValue::Null,
                ("BIGINT" | "INT" | "TINYINT", value) => SqlValue::Number(
                    value
                        .as_i64()
                        .ok_or("调度数据库整数列必须为明确整数")?
                        .to_string(),
                ),
                ("DATETIME", value) => {
                    SqlValue::Text(mysql_datetime_micros(database_time(value)?)?)
                }
                ("CHAR" | "VARCHAR", Value::String(value)) => SqlValue::Text(value.clone()),
                _ => return Err("调度完整行列类型与控制库 catalog 不一致".into()),
            };
            Ok((name.clone(), value))
        })
        .collect::<Result<Row>>()?;
    Ok((current, row))
}

fn database_time(value: &Value) -> Result<UtcOrderKey> {
    let value = document::text(value, "数据库 UTC 时间")?;
    let seconds = value.split_once('.').map_or(value, |(seconds, _)| seconds);
    if seconds.starts_with("0000") || seconds.ends_with(":60") {
        return Err("调度核对只接受数据库 UTC 的无时区 DATETIME".into());
    }
    let parsed = mysql_datetime_key(value)?;
    if mysql_datetime_micros(parsed)?.get(..19) != Some(seconds) {
        return Err("数据库 UTC 日期必须使用明确的日期与空格分隔格式".into());
    }
    Ok(parsed)
}

fn disabled(old: &Row, current: &Row, started: UtcOrderKey, finished: UtcOrderKey) -> Result<bool> {
    let version = old["version"]
        .integer()
        .and_then(|value| value.checked_add(1))
        .ok_or("原调度版本无法递增")?;
    let updated = match &current["updated_at"] {
        SqlValue::Text(value) => mysql_datetime_key(value)?,
        _ => return Ok(false),
    };
    Ok(current["enabled"].integer() == Some(0)
        && current["next_run_at"].is_null()
        && current["version"].integer() == Some(version)
        && (started..=finished).contains(&updated)
        && old.iter().all(|(name, value)| {
            CHANGED_FIELDS.contains(&name.as_str()) || &current[name] == value
        }))
}

pub(in crate::recovery::clone::native) fn observe(
    inputs: &StorageInputs<'_>,
    execution: &Path,
    target_scope: &str,
    action: &Value,
    transport: &impl Transport,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Value> {
    checkpoint()?;
    let action = action_input(action)?;
    let columns = columns(execution)?;
    let query = Query::new(
        inputs,
        execution,
        target_scope,
        CloneSide::Target,
        temporary(inputs)?,
    )?;
    let check = || {
        checkpoint()?;
        query.check(inputs, execution)
    };
    let databases = model::databases(inputs)?;
    for database in databases.values() {
        verify_database(inputs, database, &query, target_scope, transport, &check)?;
    }
    let control = &databases["shared-control"];
    let sql = full_row_sql(&columns, &action)?;
    let raw = query.read(
        inputs,
        control,
        ("schedule-full-row", sql),
        transport,
        &check,
    )?;
    let row = document::parse_fragment(&raw, "目标调度完整行")?;
    let (_, observed) = observation(&row, &columns)?;
    if observed["id"].integer()
        != action["schedule_id"]
            .as_str()
            .and_then(|id| id.parse().ok())
        || observed["tenant_id"] != SqlValue::Text("system".into())
    {
        return Err("控制库完整调度行不属于原 system 目标".into());
    }
    check()?;
    Ok(row)
}

fn verify_database(
    inputs: &StorageInputs<'_>,
    database: &Value,
    query: &Query,
    scope: &str,
    transport: &impl Transport,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<()> {
    let identity = evidence::verify_query_for_side(
        inputs,
        &query.configuration,
        query.side,
        transport,
        checkpoint,
        database,
        "identity",
    )?;
    verify_identity(&identity, database)?;
    let ownership = evidence::verify_query_for_side(
        inputs,
        &query.configuration,
        query.side,
        transport,
        checkpoint,
        database,
        "ownership",
    )?;
    verify_ownership(&ownership, database, scope)
}

fn verify_identity(identity: &str, database: &Value) -> Result<()> {
    if identity.split('\t').collect::<Vec<_>>()
        != [
            document::text(&database["server_uuid"], "调度目标库 UUID")?,
            document::text(&database["database"], "调度目标物理库")?,
        ]
    {
        return Err("调度只读目标库 UUID 或物理库不同".into());
    }
    Ok(())
}

fn verify_ownership(ownership: &str, database: &Value, scope: &str) -> Result<()> {
    if model::owners(ownership)? != model::expected_owners(scope, database["kind"] == "combined") {
        return Err("调度只读目标库完整 owner 不匹配".into());
    }
    Ok(())
}

fn full_row_sql(columns: &Columns, action: &Value) -> Result<String> {
    let id: i64 = document::text(&action["schedule_id"], "调度完整行 ID")?.parse()?;
    let fields = columns
        .iter()
        .map(|(name, kind)| {
            if !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err("调度 catalog 列名无效".into());
            }
            let value = if kind == "DATETIME" {
                format!("DATE_FORMAT(`{name}`, '{SQL_UTC_FORMAT}')")
            } else {
                format!("`{name}`")
            };
            Ok(format!("'{name}',{value}"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(format!(
        "SELECT JSON_OBJECT('database_now',DATE_FORMAT(UTC_TIMESTAMP(6),'{SQL_UTC_FORMAT}'),'row',\
        (SELECT JSON_OBJECT({}) FROM sys_job_schedule WHERE tenant_id='system' AND id={id}));",
        fields.join(",")
    ))
}

struct Query {
    side: CloneSide,
    configuration: Value,
    program: PathBuf,
    private: Value,
    evidence: Value,
}

impl Query {
    fn new(
        inputs: &StorageInputs<'_>,
        execution: &Path,
        scope: &str,
        side: CloneSide,
        temporary: &Path,
    ) -> Result<Self> {
        if fs::canonicalize(inputs.execution_backend()?)? != fs::canonicalize(execution)?
            || inputs.target_scope()? != scope
        {
            return Err("调度读窗须绑定原目标 scope 和执行后端".into());
        }
        common::scope_id(scope)?;
        Ok(Self {
            side,
            configuration: configuration(inputs, execution, side)?,
            program: inputs.tool("mysql")?,
            private: directory(inputs.backend, temporary)?,
            evidence: directory(inputs.backend, inputs.observation_dir)?,
        })
    }

    fn check(&self, inputs: &StorageInputs<'_>, execution: &Path) -> Result<()> {
        if configuration(inputs, execution, self.side)? != self.configuration
            || inputs.tool("mysql")? != self.program
        {
            return Err("调度只读配置或 MySQL 工具在观察期间变化".into());
        }
        let temporary = common::directory_identity(inputs.backend, &self.private)?;
        let evidence = common::directory_identity(inputs.backend, &self.evidence)?;
        for root in [temporary, evidence] {
            match fs::symlink_metadata(root.join("unused-login.cnf")) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
                Ok(_) => return Err("调度 MySQL 禁止读取隐式登录配置".into()),
            }
        }
        Ok(())
    }

    fn read(
        &self,
        inputs: &StorageInputs<'_>,
        control: &Value,
        command: (&str, String),
        transport: &impl Transport,
        checkpoint: &impl Fn() -> Result<()>,
    ) -> Result<String> {
        let (label, sql) = command;
        let path = Path::new(document::text(
            &control["defaults_file"],
            "调度控制库凭据文件",
        )?);
        let held = Inputs::open([common::common_file(inputs.backend, path, "调度控制库凭据")?])?;
        defaults_connection(inputs.backend, control)?;
        let work = common::directory_identity(inputs.backend, &self.private)?;
        let mut args = evidence::mysql_client_arguments(control)?;
        args.extend(["--execute".to_owned(), sql]);
        let mut environment = inputs.command_environment();
        environment.retain(|key, _| {
            !key.eq_ignore_ascii_case("MYSQL_PWD")
                && !key.eq_ignore_ascii_case("MYSQL_TEST_LOGIN_FILE")
        });
        environment.insert(
            "MYSQL_TEST_LOGIN_FILE".into(),
            common::path_text(&work.join("unused-login.cnf"), "调度禁用隐式登录")?,
        );
        checkpoint()?;
        let result = transport.run(CommandSpec {
            program: self.program.clone(),
            args,
            cwd: inputs.execution_backend()?,
            environment,
            stdin: None,
            stdout: StdoutTarget::Capture,
            work_dir: work,
            label: label.to_owned(),
            timeout: Duration::from_secs(1800),
        });
        let checked = held.check().and_then(|()| checkpoint());
        let status = result.as_ref().ok().map(|output| output.status);
        let evidence = common::directory_identity(inputs.backend, &self.evidence)?;
        publication::publish(
            &evidence.join(format!(
                "{label}-read-{}.json",
                uuid::Uuid::new_v4().simple()
            )),
            &json!({
                "format_version":1,"kind":format!("{label}-read"),"side":self.side.as_str(),"target_key":control["key"],
                "returncode":status.and_then(|status| status.code()),
                "checkpoint_passed":checked.is_ok(),"remote_writes":0,
            }),
        )?;
        checked?;
        let output = result.map_err(|_| "目标调度完整行只读命令失败，诊断已保留")?;
        if !output.status.success() || !output.stderr.is_empty() {
            return Err("目标调度完整行命令异常退出或输出诊断".into());
        }
        let raw =
            std::str::from_utf8(&output.stdout).map_err(|_| "目标调度完整行输出不是 UTF-8")?;
        Ok(raw.trim().to_owned())
    }
}

fn configuration(inputs: &StorageInputs<'_>, execution: &Path, side: CloneSide) -> Result<Value> {
    if side == CloneSide::Source {
        let bound = &inputs.target()["s3"]["configuration"];
        let directory = Path::new(document::text(&bound["directory"], "原源应用配置目录")?);
        let observed = app_configuration::configuration_value(inputs.backend, directory, "test")?;
        if observed != *bound
            || inputs.environment.get("APP_ENV").map(String::as_str) != Some("test")
        {
            return Err("管理员来源配置与已登记原文件不同".into());
        }
        return Ok(observed);
    }
    let (directory, files) =
        request_configuration::fresh_files(execution, inputs.environment, inputs.request)?;
    if inputs.request["configuration_sha256"]
        != request_configuration::configuration_identity(&directory, &files)?
    {
        return Err("调度只读配置与原目标摘要不同".into());
    }
    app_configuration::configuration_value(execution, &directory, "test")
}

fn temporary<'a>(inputs: &StorageInputs<'a>) -> Result<&'a Path> {
    match (
        inputs.environment.get("TEMP"),
        inputs.environment.get("TMP"),
    ) {
        (Some(first), Some(second)) if first == second => Ok(Path::new(first)),
        (None, None) => Ok(inputs.observation_dir),
        _ => Err("调度 TEMP/TMP 必须属于同一登记目录".into()),
    }
}

fn directory(backend: &Path, path: &Path) -> Result<Value> {
    let path = common::local_directory(backend, path, "调度只读受管目录")?;
    let (device, inode) = common::device_inode_for(&path, &fs::symlink_metadata(&path)?)?;
    Ok(json!({"path":path,"device":device,"inode":inode}))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/post_process/schedule_images.rs"]
pub(super) mod tests;
