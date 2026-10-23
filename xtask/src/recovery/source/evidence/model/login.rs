use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::{Value, json};

use super::{documents, subject_identity};
use crate::Result;

fn instant(value: &Value) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str().ok_or("来源验收时间必须带时区")?)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| "来源验收时间必须是有效的带时区时间".into())
}

pub(in crate::recovery::source::evidence) fn login_audit(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &[
            "old_before",
            "old_after",
            "upper_id",
            "new_rows",
            "lineage",
            "subjects",
            "started_at",
            "completed_at",
        ],
        "来源登录审计参数",
    )?;
    let upper = args["upper_id"].as_u64().ok_or("来源旧登录上界无效")?;
    let rows = documents::array(&args["new_rows"], "来源新增登录日志")?;
    if args["old_before"].as_str().is_none_or(|value| {
        value.len() % 2 != 0
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) || args["old_before"] != args["old_after"]
        || rows.len() != 11
    {
        return Err("来源验收登录日志旧行或新增数量不可信".into());
    }
    let begin = instant(&args["started_at"])?;
    let end = instant(&args["completed_at"])?;
    if begin > end {
        return Err("来源验收登录日志时间顺序不成立".into());
    }
    let tenants = documents::array(&args["lineage"]["tenants"], "来源租户")?;
    let subjects = documents::array(&args["subjects"], "来源主体")?;
    if tenants.len() != 11 || subjects.len() != 11 {
        return Err("来源验收登录日志缺少十一租户身份".into());
    }
    let mut expected = BTreeMap::new();
    for (index, (tenant, subject)) in tenants.iter().zip(subjects).enumerate() {
        let (id, _) = subject_identity(subject)?;
        if tenant["tenant_id"] != id
            || expected
                .insert(
                    id,
                    (&tenant["username"], format!("198.18.20.{}", index + 1)),
                )
                .is_some()
        {
            return Err("来源验收登录日志租户身份重复或不匹配".into());
        }
    }
    let mut seen = BTreeSet::new();
    let mut previous = upper;
    for row in rows {
        documents::exact(
            row,
            &[
                "id",
                "tenant_id",
                "user_name",
                "ipaddr",
                "login_location",
                "browser",
                "os",
                "status",
                "msg",
                "login_time",
            ],
            "来源验收登录日志",
        )?;
        let id_text = row["id"].as_str().ok_or("来源登录日志主键无效")?;
        if id_text.is_empty()
            || id_text.starts_with('0')
            || !id_text.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("来源登录日志主键不是规范正整数文本".into());
        }
        let id = id_text.parse::<u64>().map_err(|_| "来源登录日志主键无效")?;
        let tenant = row["tenant_id"].as_str().ok_or("来源登录日志租户无效")?;
        let (username, address) = expected.get(tenant).ok_or("来源登录日志未知租户")?;
        let logged = NaiveDateTime::parse_from_str(
            row["login_time"].as_str().ok_or("来源登录日志时间无效")?,
            "%Y-%m-%dT%H:%M:%S%.f",
        )
        .map_err(|_| "来源登录日志时间无效")?
        .and_utc();
        if id <= previous
            || !seen.insert(tenant)
            || &row["user_name"] != *username
            || row["ipaddr"] != *address
            || row["status"] != "1"
            || !row["msg"].is_null()
            || logged < begin
            || logged > end
            || ["login_location", "browser", "os"]
                .iter()
                .any(|field| !row[*field].is_null() && !row[*field].is_string())
        {
            return Err("来源验收新增登录日志与本次十一主体不同".into());
        }
        previous = id;
    }
    if expected.keys().any(|key| !seen.contains(key)) {
        return Err("来源验收新增登录日志未精确覆盖十一租户".into());
    }
    Ok(json!({"old_rows_unchanged":true,"upper_id":upper.to_string(),"new_rows":rows}))
}
