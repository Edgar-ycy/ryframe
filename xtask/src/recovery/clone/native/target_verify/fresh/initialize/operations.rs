//! 保持四库intent、sentinel、reset和十迁移原顺序；失败不删除已创建资源。
use super::super::super::super::{export::transport::Transport, json as document};
use super::super::{
    super::{creation_plan, evidence_reset, reset, resume_prefix},
    receipts,
};
use super::{reset_configuration::Snapshot, window::Window};
use crate::Result;
use crate::utc_timestamp::now_rfc3339;
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

pub(crate) fn database<T: Transport>(w: &Window<'_, T>, db: &Value, empty: bool) -> Result<Value> {
    let name = document::text(&db["database"], "数据库名")?;
    let output=w.mysql(db,&format!("SELECT @@server_uuid; SELECT SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME = '{name}';"))?;
    if output.trim().lines().collect::<Vec<_>>()
        != [document::text(&db["server_uuid"], "数据库UUID")?, name]
    {
        return Err("目标数据库UUID或精确存在性不符".into());
    }
    if empty && !w.mysql(db,&format!("SELECT TABLE_NAME FROM information_schema.TABLES WHERE TABLE_SCHEMA = '{name}' ORDER BY TABLE_NAME;"))?.trim().is_empty() {
        return Err("新目标库已出现表或view；禁止接管或重放".into());
    }
    Ok(json!({"server_uuid":db["server_uuid"],"database":name,"exists":true,"empty_checked":empty}))
}
pub(super) fn reset_plan<T: Transport>(
    w: &Window<'_, T>,
    original: &Value,
    label: &str,
) -> Result<Value> {
    state_directory(w, original)?;
    let snapshot = Snapshot::open(
        w.inputs.backend,
        w.inputs.observation_dir,
        w.inputs.request,
        &original["maintenance"],
    )?;
    let request = snapshot.request(w.inputs.request)?;
    let command = resume_prefix::configured_command(
        w.inputs
            .maintenance_executable("reset")?
            .to_str()
            .ok_or("reset路径不是UTF8")?,
        &request,
        vec!["plan".into()],
    )?
    .into_iter()
    .map(Value::String)
    .collect::<Vec<_>>();
    let raw = w.command(label, &command, 30)?;
    snapshot.check()?;
    let result = creation_plan::execute(
        w.inputs.backend,
        w.inputs.observation_dir,
        "reset-plan-output",
        &json!({"text":raw,"request":w.inputs.request,"original":original}),
    )?;
    receipts::write(
        &w.inputs.observation_dir.join(format!("{label}.json")),
        &result,
    )?;
    Ok(result)
}
fn state_directory<T: Transport>(w: &Window<'_, T>, original: &Value) -> Result<PathBuf> {
    let value = creation_plan::execute(
        w.inputs.backend,
        w.inputs.observation_dir,
        "reset-execution-configuration",
        &json!({"execution_backend":w.inputs.execution_backend()?,"app_env":w.inputs.environment.get("APP_ENV"),"maintenance":original["maintenance"]}),
    )?;
    Ok(document::text(&value["path"], "reset状态目录")?.into())
}
fn record<T: Transport>(w: &Window<'_, T>, stage: &str, suffix: &str, value: &Value) -> Result<()> {
    receipts::write(
        &w.inputs
            .observation_dir
            .join(format!("{stage}.{suffix}.json")),
        value,
    )
}
fn intent<T: Transport>(w: &Window<'_, T>, stage: &str, resource: &Value) -> Result<()> {
    record(
        w,
        stage,
        "intent",
        &json!({"at":now_rfc3339(),"stage":stage,"resource":resource}),
    )
}
fn confirmed<T: Transport>(w: &Window<'_, T>, stage: &str, observed: &Value) -> Result<()> {
    record(
        w,
        stage,
        "confirmed",
        &json!({"at":now_rfc3339(),"stage":stage,"observed":observed}),
    )
}
pub(super) fn initialize<T: Transport>(
    w: &Window<'_, T>,
    original: &Value,
    snapshot: &Snapshot,
) -> Result<Value> {
    let mut databases = document::array(&w.inputs.request["target"]["databases"], "四目标")?
        .iter()
        .collect::<Vec<_>>();
    databases.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
    for db in databases {
        w.unchanged(original)?;
        snapshot.check()?;
        let (stage, observed) = create_database(w, db)?;
        w.unchanged(original)?;
        confirmed(w, &stage, &observed)?;
    }
    w.objects(false)?;
    w.redis_state(original, false, false)?;
    snapshot.check()?;
    intent(
        w,
        "sentinel",
        &json!({"key":w.inputs.request["reset"]["sentinel_key"]}),
    )?;
    if w.redis(
        original,
        &[
            "SET",
            document::text(&w.inputs.request["reset"]["sentinel_key"], "sentinel key")?,
            document::text(&w.inputs.request["reset"]["sentinel_value"], "sentinel值")?,
            "NX",
        ],
    )? != "OK"
    {
        return Err("Redis sentinel NX未确认；禁止覆盖和重放".into());
    }
    confirmed(w, "sentinel", &w.redis_state(original, false, true)?)?;
    for db in document::array(&w.inputs.request["target"]["databases"], "四目标")? {
        database(w, db, true)?;
    }
    execute_reset(w, original)
}
pub(super) fn create_database<T: Transport>(
    w: &Window<'_, T>,
    db: &Value,
) -> Result<(String, Value)> {
    let stage = format!("create-{}", document::text(&db["key"], "数据库key")?);
    intent(
        w,
        &stage,
        &json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]}),
    )?;
    super::super::resources::database_absence(&w.inputs, w.transport, db)?;
    w.mysql(
        db,
        &format!(
            "CREATE DATABASE `{}` CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci;",
            document::text(&db["database"], "数据库名")?
        ),
    )?;
    let observed = database(w, db, true)?;
    Ok((stage, observed))
}
fn execute_reset<T: Transport>(w: &Window<'_, T>, original: &Value) -> Result<Value> {
    let plan = document::read(&w.inputs.observation_dir.join("reset-plan.json"))?;
    w.unchanged(original)?;
    if reset_plan(w, original, "reset-plan-confirm")? != plan {
        return Err("reset plan即时复核变化".into());
    }
    fs::create_dir(state_directory(w, original)?)?;
    w.unchanged(original)?;
    for db in document::array(&w.inputs.request["target"]["databases"], "四目标")? {
        database(w, db, true)?;
    }
    w.objects(false)?;
    w.redis_state(original, false, true)?;
    intent(w, "reset", &json!({"plan_hash":plan["plan_hash"]}))?;
    let snapshot = Snapshot::open(
        w.inputs.backend,
        w.inputs.observation_dir,
        w.inputs.request,
        &original["maintenance"],
    )?;
    let request = snapshot.request(w.inputs.request)?;
    let command = resume_prefix::configured_command(
        w.inputs
            .maintenance_executable("reset")?
            .to_str()
            .ok_or("reset路径不是UTF8")?,
        &request,
        vec![
            "execute".into(),
            "--plan-hash".into(),
            document::text(&plan["plan_hash"], "reset hash")?.into(),
            "--confirm-reset".into(),
            document::text(&plan["manifest"]["confirmation_phrase"], "reset确认")?.into(),
        ],
    )?
    .into_iter()
    .map(Value::String)
    .collect::<Vec<_>>();
    w.command("reset-execute", &command, 1800)?;
    snapshot.check()?;
    let mut done = reset::completed(
        w.inputs.backend,
        w.inputs.observation_dir,
        &plan["manifest"],
        document::text(&plan["plan_hash"], "reset hash")?,
    )?;
    done["configuration"] = snapshot.binding()?;
    confirmed(w, "reset", &done)?;
    let executable = w.inputs.maintenance_executable("migrate")?;
    let operations = resume_prefix::operations(
        executable.to_str().ok_or("迁移工具路径不是UTF8")?,
        w.inputs.request,
    )?;
    for operation in document::array(&operations["operations"], "十迁移")? {
        w.unchanged(original)?;
        w.command(
            document::text(&operation["stage"], "迁移阶段")?,
            document::array(&operation["command"], "迁移argv")?,
            1800,
        )?;
    }
    evidence_reset::aggregate(
        w.inputs.backend,
        w.inputs.observation_dir,
        w.inputs.request,
        original,
    )?;
    Ok(done)
}
