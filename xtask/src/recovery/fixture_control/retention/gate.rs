//! 只持有已登记隔离目标的行锁，不更改产品迁移状态。
use super::{
    super::{artifact::runtime::Binding, text_field},
    database::{self, query},
};
use crate::{Result, cli::FixtureRetentionOptions, devex::job_evidence::protocol::Session};
use serde_json::{Value, json};
use std::{
    io::{BufRead, Write},
    path::Path,
    time::{Duration, Instant},
};

pub(super) fn run(root: &Path, options: &FixtureRetentionOptions, binding: &Binding) -> Result<()> {
    let [control, target] = database::gate_bindings(root, &binding.table, &binding.scope)?;
    let mut observer = control.open()?;
    let mut gate = target.open()?;
    let result = (|| {
        let server =
            database::ownership(&mut observer, &control.schema, &binding.scope, "control")?;
        if database::ownership(&mut gate, &target.schema, &binding.scope, "tenant-data")? != server
        {
            return Err("复制门两个目标的 MySQL server UUID 不一致".into());
        }
        let contract = json!({"scope_id":binding.scope,"tenant_id":options.tenant,"migration_id":options.migration.to_string()});
        hold(
            &mut observer,
            &mut gate,
            Selection {
                control: &control.schema,
                target: &target.schema,
                contract: &contract,
                timeout: Duration::from_secs(30),
            },
            &mut std::io::stdin().lock(),
            &mut std::io::stdout().lock(),
        )
    })();
    // EOF、客户端退出和协议错误都关闭同一连接并回滚锁事务，不驻留后台锁。
    let gate_closed = gate.close();
    let observer_closed = observer.close();
    match (result, gate_closed, observer_closed) {
        (Ok(()), Ok(()), Ok(())) => Ok(()),
        (Err(error), _, _) => Err(error),
        _ => Err("复制门连接关闭未确认；核对连接和锁现场，不能声明已释放".into()),
    }
}

pub(super) struct Selection<'a> {
    pub control: &'a str,
    pub target: &'a str,
    pub contract: &'a Value,
    pub timeout: Duration,
}

pub(super) fn hold(
    observer: &mut impl Session,
    gate: &mut impl Session,
    selection: Selection<'_>,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<()> {
    let Selection {
        control,
        target,
        contract,
        timeout,
    } = selection;
    let tenant = text_field(contract, "tenant_id", "复制门")?;
    let migration = super::model::id(contract, "migration_id")?;
    super::validate_selection(tenant, migration)?;
    super::sql::identifier(control)?;
    super::sql::identifier(target)?;
    let connection = acquire(observer, gate, control, target, tenant, migration)?;
    emit(
        output,
        contract,
        "held",
        Some(("blocking_connection_id", json!(connection))),
    )?;
    while let Some(command) = read_command(input)? {
        match command["operation"].as_str() {
            Some("release") => {
                query(gate, "ROLLBACK")?;
                emit(output, contract, "released", None)?;
                return Ok(());
            }
            Some("wait-blocked") => {
                let deadline = Instant::now() + timeout;
                loop {
                    let rows = query(
                        observer,
                        &blocked_sql(control, target, tenant, migration, connection),
                    )?;
                    if rows.len() == 1 {
                        emit(
                            output,
                            contract,
                            "blocked",
                            Some(("proof", rows[0].clone())),
                        )?;
                        break;
                    }
                    if !rows.is_empty() {
                        return Err("复制门检测到多个请求，无法证明唯一复制执行".into());
                    }
                    if Instant::now() >= deadline {
                        return Err("未观察到 Worker 在本 gate 的业务行锁上等待".into());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            _ => return Err("复制门只支持 wait-blocked 与 release".into()),
        }
    }
    Ok(())
}

fn acquire(
    observer: &mut impl Session,
    gate: &mut impl Session,
    control: &str,
    target: &str,
    tenant: &str,
    migration: i64,
) -> Result<u64> {
    let rows = query(observer, &pending_sql(control, tenant, migration))?;
    validate_pending(rows, tenant, migration)?;
    if query(
        observer,
        &format!(
            "SELECT JSON_OBJECT('source',(SELECT COUNT(*) FROM `{control}`.biz_order WHERE tenant_id='{tenant}'),'target',(SELECT COUNT(*) FROM `{target}`.biz_order WHERE tenant_id='{tenant}'))"
        ),
    )? != [json!({"source":3,"target":0})]
    {
        return Err("复制门必须有恰好三条源记录且目标数据为空".into());
    }
    let locked = query(
        gate,
        &format!(
            "SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ; START TRANSACTION; SELECT id FROM `{target}`.biz_order WHERE tenant_id='{tenant}' FOR UPDATE; SELECT JSON_OBJECT('connection_id',CONNECTION_ID())"
        ),
    )?;
    let [locked] = locked.as_slice() else {
        return Err("复制门锁连接结果不唯一".into());
    };
    locked["connection_id"]
        .as_u64()
        .filter(|v| *v > 0)
        .ok_or_else(|| "复制门锁连接 ID 无效".into())
}

fn read_command(input: &mut impl BufRead) -> Result<Option<Value>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    // 每条请求只允许 16 KiB；保留下一行给后续请求，不无界读取 stdin。
    input.take(16 * 1024 + 1).read_until(b'\n', &mut bytes)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() > 16 * 1024 {
        return Err("复制门命令超过 16 KiB".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "复制门命令必须为 UTF-8 JSON")?;
    let value = crate::recovery::clone::native::json::parse_fragment(text, "复制门命令")?;
    super::super::exact_object(&value, &["operation"], "复制门命令")?;
    Ok(Some(value))
}

fn emit(
    output: &mut impl Write,
    contract: &Value,
    state: &str,
    extra: Option<(&str, Value)>,
) -> Result<()> {
    let mut value = contract.clone();
    value["state"] = json!(state);
    if let Some((key, item)) = extra {
        value[key] = item;
    }
    serde_json::to_writer(&mut *output, &value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn validate_pending(rows: Vec<Value>, tenant: &str, migration: i64) -> Result<()> {
    let expected = json!({"migration_id":migration.to_string(),"tenant_id":tenant,"source":"shared-control","target":"shared","state":"prechecking","retention_hours":168,"job_status":"pending","attempts":0,"job_migration_id":migration.to_string()});
    if rows != [expected] {
        return Err("迁移并非本次隔离租户的未执行复制任务".into());
    }
    Ok(())
}

fn pending_sql(control: &str, tenant: &str, migration: i64) -> String {
    format!(
        "SELECT JSON_OBJECT('migration_id',CAST(m.id AS CHAR),'tenant_id',m.tenant_id,'source',m.source_target_key,'target',m.target_key,'state',m.state,'retention_hours',m.retention_hours,'job_status',j.status,'attempts',j.attempts,'job_migration_id',JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.migration_id'))) FROM `{control}`.sys_tenant_data_migration m JOIN `{control}`.sys_background_job j ON j.id=m.background_job_id WHERE m.id={migration} AND m.tenant_id='{tenant}' AND j.job_type='tenant_data_migration'"
    )
}
fn blocked_sql(
    control: &str,
    target: &str,
    tenant: &str,
    migration: i64,
    connection: u64,
) -> String {
    format!(
        "SELECT JSON_OBJECT('migration_id',CAST(m.id AS CHAR),'tenant_id',m.tenant_id,'state',m.state,'item_state',i.state,'job_status',j.status,'attempts',j.attempts,'source_rows',i.source_row_count,'target_rows',i.target_row_count,'blocking_connection_id',b.PROCESSLIST_ID,'waiting_connection_id',r.PROCESSLIST_ID,'table_name',l.OBJECT_NAME) FROM performance_schema.data_lock_waits w JOIN performance_schema.data_locks l ON l.ENGINE=w.ENGINE AND l.ENGINE_LOCK_ID=w.REQUESTING_ENGINE_LOCK_ID JOIN performance_schema.threads b ON b.THREAD_ID=w.BLOCKING_THREAD_ID JOIN performance_schema.threads r ON r.THREAD_ID=w.REQUESTING_THREAD_ID JOIN `{control}`.sys_tenant_data_migration m ON m.id={migration} AND m.tenant_id='{tenant}' JOIN `{control}`.sys_tenant_data_migration_item i ON i.migration_id=m.id AND i.table_name='biz_order' JOIN `{control}`.sys_background_job j ON j.id=m.background_job_id WHERE b.PROCESSLIST_ID={connection} AND l.OBJECT_SCHEMA='{target}' AND l.OBJECT_NAME='biz_order' AND m.state='copying' AND i.state='copying' AND j.status='running'"
    )
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/retention/gate_tests.rs"]
mod tests;
