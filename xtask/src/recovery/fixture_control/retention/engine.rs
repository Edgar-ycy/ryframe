use super::{
    super::{read_json, text_field},
    backup::{self, Exporter},
    database::query,
    model, sql,
};
use crate::{Result, devex::job_evidence::protocol::Session};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn lock_records(
    session: &mut impl Session,
    control: &str,
    contract: &Value,
) -> Result<()> {
    let migration = model::id(contract, "migration_id")?;
    let tenant = text_field(contract, "tenant_id", "历史绑定")?;
    for (index, statement) in sql::locks(control, tenant, migration)?.iter().enumerate() {
        let rows = query(session, statement)?;
        if index == 1 && rows != [json!({"id":migration.to_string()})] {
            return Err("当前租户没有唯一匹配迁移".into());
        }
    }
    Ok(())
}

fn snapshot(
    session: &mut impl Session,
    query_sql: &str,
    contract: &Value,
    finalized: bool,
) -> Result<Value> {
    model::validate(
        query(session, query_sql)?,
        text_field(contract, "tenant_id", "历史绑定")?,
        model::id(contract, "migration_id")?,
        finalized,
    )
}

pub(super) fn plan(session: &mut impl Session, query_sql: &str, contract: &Value) -> Result<Value> {
    model::plan(contract, snapshot(session, query_sql, contract, false)?)
}

pub(super) fn apply(
    session: &mut impl Session,
    control: &str,
    query_sql: &str,
    directory: &Path,
    contract: &Value,
    expected: &str,
) -> Result<Value> {
    let mut planned = plan(session, query_sql, contract)?;
    if planned["plan_sha256"] != expected {
        return Err("历史 fixture plan 与当前精确任务或时间事实不匹配，拒绝写入".into());
    }
    planned
        .as_object_mut()
        .ok_or("计划不是对象")?
        .remove("state");
    let row = planned["before"].clone();
    let migration = model::id(&row, "migration_id")?;
    // 独占持久化前像后才写数据库；异常始终保留文件，不自动重复平移。
    backup::publish(
        &directory.join(format!("business-history-{migration}.json")),
        &planned,
    )?;
    query(session, &sql::shift(control, &row)?)?;
    let after = snapshot(session, query_sql, contract, false)?;
    model::verify_shift(&row, &after)?;
    query(session, "COMMIT")?;
    planned["state"] = json!("historical-expired");
    planned["after"] = after;
    Ok(planned)
}

pub(super) fn assert_history(directory: &Path, contract: &Value, row: &Value) -> Result<()> {
    let migration = model::id(row, "migration_id")?;
    let history = read_json(
        &directory.join(format!("business-history-{migration}.json")),
        "历史调整收据",
        4 * 1024 * 1024,
    )?;
    if history["artificial_history"] != true
        || contract
            .as_object()
            .ok_or("历史绑定不是对象")?
            .iter()
            .any(|(k, v)| history.get(k) != Some(v))
    {
        return Err("历史调整收据与当前运行、数据库或迁移不匹配".into());
    }
    model::verify_shift(&history["before"], row)
}

pub(super) fn export(
    session: &mut impl Session,
    exporter: &mut impl Exporter,
    control: &str,
    target: &str,
    query_sql: &str,
    directory: &Path,
    contract: &Value,
) -> Result<Value> {
    let row = snapshot(session, query_sql, contract, false)?;
    assert_history(directory, contract, &row)?;
    let migration = model::id(&row, "migration_id")?;
    let tenant = text_field(&row, "tenant_id", "历史迁移")?;
    let scope = text_field(contract, "scope_id", "历史绑定")?;
    let provider = format!("full-stack://{scope}/historical-retention/{migration}");
    if query(
        session,
        &format!(
            "SELECT JSON_OBJECT('count',COUNT(*)) FROM `{control}`.sys_tenant_data_backup_point WHERE id={migration} OR provider_ref='{provider}'"
        ),
    )? != [json!({"count":0})]
    {
        return Err("备份 ID 或 provider_ref 已存在，拒绝覆盖".into());
    }
    let generation = model::id(&row, "target_generation")?;
    let fence = query(
        session,
        &format!(
            "SELECT JSON_OBJECT('tenant_id',tenant_id,'generation',CAST(placement_generation AS CHAR)) FROM `{target}`.biz_tenant_fence WHERE tenant_id='{tenant}' AND placement_generation={generation} AND state='active' AND switch_token=(SELECT switch_token FROM `{control}`.sys_tenant_data_migration WHERE id={migration}) FOR UPDATE"
        ),
    )?;
    if fence != [json!({"tenant_id":tenant,"generation":generation.to_string()})] {
        return Err("导出期间无法取得当前租户的精确写入 fence".into());
    }
    query(session, "SET @backup_captured_at=UTC_TIMESTAMP(6)")?;
    let artifact = exporter.export(directory, migration)?;
    let after = snapshot(session, query_sql, contract, false)?;
    assert_history(directory, contract, &after)?;
    backup::verify(&artifact, directory, migration)?;
    let mut evidence = contract.clone();
    evidence.as_object_mut().ok_or("历史绑定不是对象")?.extend(json!({"state":"backup-exported","artificial_history":true,"natural_elapsed_7_days":false,"restored":false,"artifact":artifact,"backup_id":migration.to_string(),"provider_ref":provider,"tenant_write_fence_held":true}).as_object().expect("常量对象").clone());
    backup::publish(
        &directory.join(format!("business-backup-{migration}.json")),
        &evidence,
    )?;
    query(
        session,
        &sql::register(
            control,
            &row,
            &provider,
            text_field(&artifact, "sha256", "备份文件")?,
        )?,
    )?;
    query(session, "COMMIT")?;
    Ok(evidence)
}

pub(super) fn verify_cleaned(
    session: &mut impl Session,
    query_sql: &str,
    directory: &Path,
    contract: &Value,
) -> Result<Value> {
    let row = snapshot(session, query_sql, contract, true)?;
    let migration = model::id(&row, "migration_id")?;
    let evidence = read_json(
        &directory.join(format!("business-backup-{migration}.json")),
        "历史备份收据",
        4 * 1024 * 1024,
    )?;
    if contract
        .as_object()
        .ok_or("历史绑定不是对象")?
        .iter()
        .any(|(k, v)| evidence.get(k) != Some(v))
    {
        return Err("清理验证的备份收据不匹配".into());
    }
    backup::verify(&evidence["artifact"], directory, migration)?;
    let mut result = contract.clone();
    result.as_object_mut().ok_or("历史绑定不是对象")?.extend(json!({"state":"cleaned","artificial_history":true,"natural_elapsed_7_days":false,"restored":false,"snapshot":row,"artifact":evidence["artifact"]}).as_object().expect("常量对象").clone());
    Ok(result)
}
