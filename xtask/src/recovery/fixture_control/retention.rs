use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{FixtureRetentionCommand, FixtureRetentionOptions},
    local_test_path::LocalTestPathKind,
};

#[path = "retention/backup.rs"]
mod backup;
#[path = "retention/database.rs"]
mod database;
#[path = "retention/engine.rs"]
mod engine;
#[path = "retention/gate.rs"]
mod gate;
#[path = "retention/model.rs"]
mod model;
#[path = "retention/sql.rs"]
mod sql;
#[cfg(test)]
#[path = "../../../tests/unit/recovery/fixture_control/retention/tests.rs"]
mod tests;

pub(super) fn run(command: &FixtureRetentionCommand, root: &Path) -> Result<()> {
    let options = options(command)?;
    validate_values(
        options,
        match command {
            FixtureRetentionCommand::HistoricalExpired { plan_sha256, .. } => Some(plan_sha256),
            _ => None,
        },
    )?;
    super::validate_path(
        &options.runtime_dir,
        root,
        LocalTestPathKind::ExistingDirectory,
        "历史运行目录",
    )?;
    if std::env::var("APP_ENV").as_deref() != Ok("test") {
        return Err(
            "历史保留期操作要求 APP_ENV=test；选择明确的隔离文件配置后重试；尚未连接数据库".into(),
        );
    }
    let binding = super::artifact::runtime::verify(root, &options.runtime_dir)?;
    if command.writes()
        || matches!(
            command,
            FixtureRetentionCommand::PlanHistory(_) | FixtureRetentionCommand::HoldCopy(_)
        )
    {
        require_worker_stopped(&options.runtime_dir, &binding.receipt)?;
    }
    if matches!(command, FixtureRetentionCommand::HoldCopy(_)) {
        return gate::run(root, options, &binding);
    }
    let [control, source, mut target] = database::bindings(root, &binding.table, &binding.scope)?;
    let schemas = [
        control.schema.clone(),
        source.schema.clone(),
        target.schema.clone(),
    ];
    let mut session = control.open()?;
    let result = (|| {
        let server = database::ownership(&mut session, &schemas[0], &binding.scope, "control")?;
        for schema in &schemas[1..] {
            if database::ownership(&mut session, schema, &binding.scope, "tenant-data")? != server {
                return Err("历史 fixture 的 MySQL server UUID 不一致".into());
            }
        }
        let contract = json!({"scope_id":binding.scope,"tenant_id":options.tenant,"migration_id":options.migration.to_string(),"server_uuid":server,"schemas":schemas,"configuration_sha256":binding.receipt["configuration_sha256"]});
        let query = sql::snapshot(&schemas, &options.tenant, options.migration)?;
        execute(command, &mut session, &mut target, &query, &contract)
    })();
    use crate::devex::job_evidence::protocol::Session;
    let closed = session.close().map_err(|category| {
        format!("历史会话关闭失败：{category}；保留证据，不能声明成功或自动重放")
    });
    let result = result?;
    closed?;
    if super::artifact::runtime::verify(root, &options.runtime_dir)?.receipt != binding.receipt {
        return Err("历史操作期间运行来源发生漂移；保留写入和证据，禁止自动重放".into());
    }
    super::print_json(&result)
}

fn options(command: &FixtureRetentionCommand) -> Result<&FixtureRetentionOptions> {
    match command {
        FixtureRetentionCommand::Help => Err("retention 帮助不执行操作".into()),
        FixtureRetentionCommand::Inspect(options)
        | FixtureRetentionCommand::HoldCopy(options)
        | FixtureRetentionCommand::PlanHistory(options)
        | FixtureRetentionCommand::ExportBackup(options)
        | FixtureRetentionCommand::VerifyCleaned(options)
        | FixtureRetentionCommand::HistoricalExpired { options, .. } => Ok(options),
    }
}

fn execute(
    command: &FixtureRetentionCommand,
    session: &mut impl crate::devex::job_evidence::protocol::Session,
    target: &mut database::Connection,
    query: &str,
    contract: &Value,
) -> Result<Value> {
    let options = options(command)?;
    let directory = &options.runtime_dir;
    let control = contract["schemas"][0].as_str().ok_or("缺少控制库")?;
    match command {
        FixtureRetentionCommand::Inspect(_) => {
            let mut rows = database::query(session, query)?;
            if rows.len() != 1 {
                return Err("当前租户没有唯一匹配迁移、任务、检查点".into());
            }
            let mut row = rows.remove(0);
            let attempts = row["attempt_records"]
                .as_array()
                .ok_or("迁移观察缺少任务尝试清单")?;
            let mut attempts = attempts
                .iter()
                .map(|item| model::id(item, "sequence").map(|sequence| (sequence, item.clone())))
                .collect::<Result<Vec<_>>>()?;
            attempts.sort_by_key(|(sequence, _)| *sequence);
            row["attempt_records"] = json!(
                attempts
                    .into_iter()
                    .map(|(_, item)| item)
                    .collect::<Vec<_>>()
            );
            let mut result = contract.clone();
            result["state"] = json!("observed");
            result["snapshot"] = row;
            Ok(result)
        }
        FixtureRetentionCommand::PlanHistory(_) => engine::plan(session, query, contract),
        FixtureRetentionCommand::VerifyCleaned(_) => {
            engine::verify_cleaned(session, query, directory, contract)
        }
        FixtureRetentionCommand::HistoricalExpired { plan_sha256, .. } => {
            engine::lock_records(session, control, contract)?;
            engine::apply(session, control, query, directory, contract, plan_sha256)
        }
        FixtureRetentionCommand::ExportBackup(_) => {
            engine::lock_records(session, control, contract)?;
            let schema = target.schema.clone();
            engine::export(
                session, target, control, &schema, query, directory, contract,
            )
        }
        FixtureRetentionCommand::Help => Err("帮助不执行操作".into()),
        FixtureRetentionCommand::HoldCopy(_) => Err("复制门使用独立事务通道".into()),
    }
}

fn require_worker_stopped(directory: &Path, runtime: &Value) -> Result<()> {
    let path = directory.join("worker.json");
    if !path.exists() {
        return Ok(());
    }
    let value = super::read_json(&path, "Worker 进程收据", 64 * 1024)?;
    let identity = crate::recovery::clone::native::resources::common::validate_process_identity(
        &value["identity"],
    )?;
    if value["format_version"] != 1
        || value["role"] != "worker"
        || value["scope_id"] != runtime["scope_id"]
        || identity["executable"] != runtime["artifacts"]["worker"]["path"]
    {
        return Err("Worker 进程收据与运行来源不匹配；尚未连接数据库".into());
    }
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("Worker PID 无效")?)?;
    match crate::recovery::clone::native::resources::common::process_identity(pid)? {
        None => Ok(()),
        Some(actual) if actual != identity => {
            Err("Worker PID 已复用，禁止以其他身份推进历史写入".into())
        }
        Some(_) => Err("人工历史调整和备份前必须先停止本次已登记 Worker".into()),
    }
}

fn validate_selection(tenant: &str, migration: i64) -> Result<()> {
    if !regex::Regex::new(r"^tenant-[a-f0-9]{8}$")?.is_match(tenant) || migration <= 0 {
        return Err("历史 fixture 只接受 tenant-[a-f0-9]{8} 与正 i64 迁移 ID".into());
    }
    Ok(())
}

fn validate_values(options: &FixtureRetentionOptions, plan_sha256: Option<&String>) -> Result<()> {
    let valid_tenant = options.tenant.len() == 15
        && options.tenant.starts_with("tenant-")
        && options.tenant[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
    if !valid_tenant {
        return Err("保留期租户必须符合 tenant-[a-f0-9]{8}".into());
    }
    if options.migration <= 0 {
        return Err("迁移 ID 必须是正 i64".into());
    }
    if let Some(value) = plan_sha256
        && (value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
    {
        return Err("历史计划摘要必须是 64 位小写十六进制".into());
    }
    Ok(())
}
