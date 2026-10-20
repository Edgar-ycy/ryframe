use serde_json::Value;

use super::model;

fn schema(text: &str) -> Result<&str, &'static str> {
    if text.is_empty()
        || text.len() > 64
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err("invalid_database_identifier");
    }
    Ok(text)
}

pub(crate) fn job_sql(database: &str, expected: &Value) -> Result<String, &'static str> {
    let database = schema(database)?;
    model::selection(expected)?;
    let business_id = expected["id"].as_str().ok_or("invalid_job_identifier")?;
    let tenant = expected["tenant_id"].as_str().ok_or("invalid_tenant")?;
    let (source, state) = match expected["kind"].as_str() {
        Some("export" | "import") => {
            let (table, state) = if expected["kind"] == "export" {
                ("sys_export_job", "b.status")
            } else {
                (
                    "sys_user_import_job",
                    "IF(b.cancel_requested,'cancel_requested',b.status)",
                )
            };
            (
                format!(
                    "`{database}`.`{table}` b JOIN `{database}`.sys_background_job j ON j.id=b.background_job_id"
                ),
                state,
            )
        }
        Some("schedule") => (
            format!(
                "`{database}`.sys_job_schedule_execution b JOIN `{database}`.sys_background_job j \
                 ON j.id=b.background_job_id AND j.schedule_id=b.schedule_id \
                 AND JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.schedule_id'))=CAST(b.schedule_id AS CHAR)"
            ),
            "b.outcome",
        ),
        Some("message") => (
            format!(
                "`{database}`.sys_message b JOIN `{database}`.sys_outbox_event o \
                 ON o.aggregate_type='message' AND o.aggregate_id=CAST(b.id AS CHAR) \
                 AND o.tenant_id=b.tenant_id AND o.event_type='system.message.published' \
                 JOIN `{database}`.sys_background_job j ON j.dedupe_key=o.dedupe_key \
                 AND j.dedupe_key=CONCAT('message:',b.id) \
                 AND JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.message_id'))=CAST(b.id AS CHAR)"
            ),
            "o.status",
        ),
        _ => return Err("invalid_job_kind"),
    };
    Ok(format!(
        "SELECT JSON_OBJECT('business_id',CAST(b.id AS CHAR),'business_tenant',b.tenant_id,\
         'business_status',{state},'job_id',CAST(j.id AS CHAR),'job_tenant',j.tenant_id,\
         'job_type',j.job_type,'status',j.status,'claim_sequence',j.claim_sequence) \
         FROM {source} WHERE b.id={business_id} AND b.tenant_id='{tenant}' LIMIT 2"
    ))
}

pub(crate) fn attempts_sql(database: &str, job_id: &str) -> Result<String, &'static str> {
    let database = schema(database)?;
    model::identifier(&Value::String(job_id.to_owned()))?;
    let times = ["available_at", "started_at", "finished_at", "closed_at"]
        .iter()
        .map(|name| format!("'{name}',DATE_FORMAT(`{name}`,'%Y-%m-%dT%H:%i:%s.%fZ')"))
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!(
        "SELECT JSON_OBJECT('job_id',CAST(job_id AS CHAR),'sequence',`sequence`,\
         'outcome',outcome,{times}) FROM `{database}`.sys_background_job_attempt \
         WHERE job_id={job_id} ORDER BY `sequence` LIMIT 10001"
    ))
}
