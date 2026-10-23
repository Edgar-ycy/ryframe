use super::model::{self, ATTEMPT_TIMES, ITEM_TIMES, JOB_TIMES, MIGRATION_TIMES, SHIFT_HOURS};
use crate::Result;
use serde_json::Value;

pub(super) fn identifier(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("测试数据库标识无效".into());
    }
    Ok(value)
}

pub(super) fn snapshot(schemas: &[String; 3], tenant: &str, migration: i64) -> Result<String> {
    let mut sql = include_str!("snapshot.sql").to_owned();
    for (key, value) in ["control", "source", "target"].iter().zip(schemas) {
        identifier(value)?;
        sql = sql.replace(&format!("{{{key}}}"), value);
    }
    Ok(sql
        .replace("{tenant}", tenant)
        .replace("{migration}", &migration.to_string()))
}

pub(super) fn shift(control: &str, row: &Value) -> Result<String> {
    identifier(control)?;
    let attempts = model::attempts(row)?;
    let mut statements = Vec::new();
    for (table, key, columns) in [
        ("sys_tenant_data_migration", "migration_id", MIGRATION_TIMES),
        ("sys_tenant_data_migration_item", "item_id", ITEM_TIMES),
        ("sys_background_job", "job_id", JOB_TIMES),
    ] {
        let id = model::id(row, key)?;
        let assignments = columns
            .iter()
            .map(|column| format!("{column}=DATE_SUB({column}, INTERVAL {SHIFT_HOURS} HOUR)"))
            .collect::<Vec<_>>()
            .join(",");
        statements.push(format!(
            "UPDATE `{control}`.{table} SET {assignments} WHERE id={id}"
        ));
    }
    let assignments = ATTEMPT_TIMES
        .iter()
        .map(|column| format!("a.{column}=DATE_SUB(a.{column}, INTERVAL {SHIFT_HOURS} HOUR)"))
        .collect::<Vec<_>>()
        .join(",");
    let sequences = attempts
        .iter()
        .map(|item| model::id(item, "sequence").map(|v| v.to_string()))
        .collect::<Result<Vec<_>>>()?
        .join(",");
    let migration = model::id(row, "migration_id")?;
    let job = model::id(row, "job_id")?;
    let tenant = super::super::text_field(row, "tenant_id", "迁移")?;
    super::validate_selection(tenant, migration)?;
    statements.push(format!("UPDATE `{control}`.sys_background_job_attempt a JOIN `{control}`.sys_background_job j ON j.id=a.job_id JOIN `{control}`.sys_tenant_data_migration m ON m.background_job_id=j.id SET {assignments} WHERE m.id={migration} AND m.tenant_id='{tenant}' AND j.id={job} AND j.tenant_id='{tenant}' AND j.job_type='tenant_data_migration' AND a.sequence IN ({sequences})"));
    Ok(statements.join(";\n"))
}

pub(super) fn locks(control: &str, tenant: &str, migration: i64) -> Result<Vec<String>> {
    identifier(control)?;
    super::validate_selection(tenant, migration)?;
    Ok(vec![
        "SET SESSION time_zone='+00:00'; START TRANSACTION".into(),
        format!(
            "SELECT JSON_OBJECT('id',CAST(id AS CHAR)) FROM `{control}`.sys_tenant_data_migration WHERE id={migration} AND tenant_id='{tenant}' FOR UPDATE"
        ),
        format!(
            "SELECT JSON_OBJECT('id',CAST(j.id AS CHAR)) FROM `{control}`.sys_background_job j JOIN `{control}`.sys_tenant_data_migration m ON m.background_job_id=j.id WHERE m.id={migration} AND m.tenant_id='{tenant}' FOR UPDATE"
        ),
        format!(
            "SELECT JSON_OBJECT('id',CAST(id AS CHAR)) FROM `{control}`.sys_tenant_data_migration_item WHERE migration_id={migration} FOR UPDATE"
        ),
        format!(
            "SELECT JSON_OBJECT('job_id',CAST(a.job_id AS CHAR),'sequence',CAST(a.sequence AS CHAR)) FROM `{control}`.sys_background_job_attempt a JOIN `{control}`.sys_background_job j ON j.id=a.job_id AND j.job_type='tenant_data_migration' JOIN `{control}`.sys_tenant_data_migration m ON m.background_job_id=j.id WHERE m.id={migration} AND m.tenant_id='{tenant}' AND j.tenant_id='{tenant}' ORDER BY a.sequence FOR UPDATE"
        ),
    ])
}

pub(super) fn register(
    control: &str,
    row: &Value,
    provider: &str,
    checksum: &str,
) -> Result<String> {
    identifier(control)?;
    super::super::valid_sha256(checksum, "备份摘要")?;
    if provider.contains(['\'', '\\', '\0']) {
        return Err("备份 provider_ref 无效".into());
    }
    let migration = model::id(row, "migration_id")?;
    let tenant = super::super::text_field(row, "tenant_id", "备份")?;
    super::validate_selection(tenant, migration)?;
    Ok(format!("INSERT INTO `{control}`.sys_tenant_data_backup_point
        (id,scope,tenant_id,target_key,placement_generation,schema_fingerprint,provider_ref,
         captured_at,checksum,validation_status,validation_detail,retention_until,expires_at,created_by,created_at,updated_at)
        SELECT m.id,'tenant',m.tenant_id,m.target_key,m.target_generation,m.target_schema_fingerprint,
          '{provider}',@backup_captured_at,'{checksum}','valid',
          '历史保留期测试：迁移日期人工平移；目标由 mysqldump 真实导出并校验摘要，未执行恢复验证',
          DATE_ADD(@backup_captured_at,INTERVAL 7 DAY),DATE_ADD(@backup_captured_at,INTERVAL 7 DAY),
          m.operator_id,UTC_TIMESTAMP(6),UTC_TIMESTAMP(6)
        FROM `{control}`.sys_tenant_data_migration m WHERE m.id={migration} AND m.tenant_id='{tenant}'"))
}
