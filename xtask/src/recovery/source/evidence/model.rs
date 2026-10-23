//! 来源读取只允许当前合同中的会话写入，其他前后像必须完全一致。
use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde_json::{Value, json};

use super::documents;
use crate::{Result, recovery::clone::native::digest};

#[path = "model/login.rs"]
mod login;
pub(super) use login::login_audit;

const SUBJECT: &[&str] = &["tenant_id", "user_id", "user_authorization_version"];
const DELTAS: &[(&str, u64)] = &[
    ("sys_login_info", 11),
    ("sys_oper_log", 0),
    ("sys_outbox_event", 0),
];

pub(super) fn result(args: &Value) -> Result<Value> {
    documents::exact(
        args,
        &["value", "lineage", "start", "lineage_descriptor"],
        "来源结果校验参数",
    )?;
    let value = &args["value"];
    let lineage = &args["lineage"];
    for path in ["/verification/scope_id", "/scopes/origin_tenant_scope_id"] {
        if lineage.pointer(path).and_then(Value::as_str).is_none() {
            return Err("来源结果缺少明确血缘scope".into());
        }
    }
    for field in ["tenants", "post_samples", "business_objects"] {
        if lineage["scale"][field].as_u64().is_none() {
            return Err("来源结果缺少完整业务规模".into());
        }
    }
    for field in ["start", "lineage_descriptor"] {
        if args[field]["sha256"].as_str().is_none_or(|value| {
            value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        }) {
            return Err("来源结果缺少明确start或血缘文件摘要".into());
        }
    }
    documents::exact(
        value,
        &[
            "format_version",
            "kind",
            "status",
            "scope_id",
            "origin_tenant_scope_id",
            "lineage_sha256",
            "actions",
            "restore_success",
            "subjects",
            "tenants",
            "posts",
            "files",
            "source_generation_sha256",
            "lineage_file_sha256",
        ],
        "来源原生验收结果",
    )?;
    let expected = json!({
        "format_version":1,"kind":"restore-source-existing-verification",
        "status":"source_existing_data_verified","scope_id":lineage["verification"]["scope_id"],
        "origin_tenant_scope_id":lineage["scopes"]["origin_tenant_scope_id"],
        "lineage_sha256":digest::plan_hash(lineage)?,
        "actions":{"business":"read_only","objects":"read_only","session":"login_logout"},
        "restore_success":false,"tenants":lineage["scale"]["tenants"],
        "posts":lineage["scale"]["post_samples"],"files":lineage["scale"]["business_objects"],
        "source_generation_sha256":args["start"]["sha256"],
        "lineage_file_sha256":args["lineage_descriptor"]["sha256"],
    });
    if expected
        .as_object()
        .expect("固定对象")
        .iter()
        .any(|(key, item)| &value[key] != item)
    {
        return Err("来源原生验收没有绑定同一start、血缘或完整业务样本".into());
    }
    let subjects = documents::array(&value["subjects"], "来源登录主体")?;
    let tenants = documents::array(&lineage["tenants"], "派生来源租户")?;
    if subjects.len() != 11 || tenants.len() != 11 {
        return Err("来源原生验收缺少十一租户的登录主体".into());
    }
    let mut identities = BTreeSet::new();
    for (subject, tenant) in subjects.iter().zip(tenants) {
        let (tenant_id, user) = subject_identity(subject)?;
        if subject["tenant_id"] != tenant["tenant_id"] || !identities.insert((tenant_id, user)) {
            return Err("来源登录主体与派生租户身份不同".into());
        }
    }
    Ok(value.clone())
}

fn subject_identity(subject: &Value) -> Result<(&str, &str)> {
    documents::exact(subject, SUBJECT, "来源登录主体")?;
    let tenant = subject["tenant_id"]
        .as_str()
        .ok_or("来源登录主体租户无效")?;
    let user = subject["user_id"].as_str().ok_or("来源登录主体ID无效")?;
    if !Regex::new(r"^[a-z0-9][a-z0-9_-]{2,47}$")?.is_match(tenant)
        || !Regex::new(r"^[1-9][0-9]{0,18}$")?.is_match(user)
        || user.parse::<i64>().is_err()
        || subject["user_authorization_version"]
            .as_u64()
            .is_none_or(|value| value > i32::MAX as u64)
    {
        return Err("来源登录主体不能生成唯一授权缓存键".into());
    }
    Ok((tenant, user))
}

pub(super) fn cache_plan(args: &Value) -> Result<Value> {
    documents::exact(args, &["namespace", "subjects"], "来源缓存计划参数")?;
    let namespace = args["namespace"]
        .as_str()
        .ok_or("来源Redis namespace无效")?;
    if !Regex::new(r"^ryframe:\{[a-z0-9][a-z0-9_-]{2,47}\}:$")?.is_match(namespace) {
        return Err("来源Redis namespace不是已登记scope的规范前缀".into());
    }
    let mut rows = Vec::new();
    let mut identities = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for subject in documents::array(&args["subjects"], "来源登录主体")? {
        let (tenant, user) = subject_identity(subject)?;
        if !identities.insert((tenant, user)) {
            return Err("来源登录主体重复".into());
        }
        let prefix = format!("{namespace}ryframe:authorization:{{{tenant}}}");
        let tenant_epoch = format!("{prefix}:epoch");
        let version = format!("{prefix}:user:{user}:version");
        let snapshot = format!("{prefix}:user:{user}:snapshots");
        keys.extend([tenant_epoch.clone(), version.clone(), snapshot.clone()]);
        rows.push(json!({"tenant_id":tenant,"user_id":user,
            "user_authorization_version":subject["user_authorization_version"],
            "keys":{"tenant_epoch":tenant_epoch,"user_version":version,"snapshot":snapshot}}));
    }
    if rows.len() != 11 || keys.len() != 33 {
        return Err("来源验收必须精确绑定十一主体的33个授权缓存键".into());
    }
    Ok(Value::Array(rows))
}

fn table_map(database: &Value) -> Result<BTreeMap<String, Value>> {
    let mut result = BTreeMap::new();
    let digest = Regex::new(r"^[a-f0-9]{64}$")?;
    for row in documents::array(
        &database["target"]["database"]["tables"],
        "完整数据库像业务表",
    )? {
        documents::exact(row, &["table", "rows", "sha256"], "完整数据库像业务表")?;
        let name = row["table"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or("数据库表身份无效")?;
        let hash = row["sha256"].as_str().ok_or("数据库表摘要无效")?;
        if row["rows"].as_u64().is_none()
            || !digest.is_match(hash)
            || result.insert(name.into(), row.clone()).is_some()
        {
            return Err("数据库表身份或摘要无效".into());
        }
    }
    Ok(result)
}

pub(super) fn write_effects(args: &Value) -> Result<Value> {
    documents::exact(args, &["before", "after"], "来源前后像参数")?;
    let before = &args["before"];
    let after = &args["after"];
    let fields = &[
        "databases",
        "schema",
        "objects",
        "owners",
        "redis",
        "storage",
    ];
    documents::exact(before, fields, "来源前像")?;
    documents::exact(after, fields, "来源后像")?;
    for field in &fields[1..] {
        if before[*field] != after[*field] {
            return Err(format!("来源验收期间{field}发生未登记变化").into());
        }
    }
    let first = documents::object(&before["databases"], "来源前像数据库")?;
    let second = documents::object(&after["databases"], "来源后像数据库")?;
    if first.keys().ne(second.keys()) || !first.contains_key("shared-control") {
        return Err("来源验收前后像没有同一完整四库集合".into());
    }
    for key in first.keys().filter(|key| key.as_str() != "shared-control") {
        if first[key] != second[key] {
            return Err("来源验收改变了租户业务数据库".into());
        }
    }
    let mut first = first["shared-control"].clone();
    let mut second = second["shared-control"].clone();
    let mut old = table_map(&first)?;
    let mut new = table_map(&second)?;
    if old.keys().ne(new.keys()) || DELTAS.iter().any(|(name, _)| !old.contains_key(*name)) {
        return Err("来源验收控制库表集合或会话审计表不完整".into());
    }
    let preserved = &first["target"]["preserved_tables"];
    if preserved != &second["target"]["preserved_tables"]
        || preserved.as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| DELTAS.iter().any(|(name, _)| row["table"] == *name))
        })
    {
        return Err("来源验收不能把会话审计写入伪装为保留表".into());
    }
    for &(name, delta) in DELTAS {
        let previous = old.remove(name).expect("表集合已核对");
        let current = new.remove(name).expect("表集合已核对");
        if previous["rows"]
            .as_u64()
            .and_then(|rows| rows.checked_add(delta))
            != current["rows"].as_u64()
            || (delta == 0 && previous != current)
            || (delta > 0 && previous["sha256"] == current["sha256"])
        {
            return Err(format!("来源验收{name}行数或摘要变化与当前产品合同不同").into());
        }
    }
    first["target"]["database"]["tables"] = json!([]);
    second["target"]["database"]["tables"] = json!([]);
    if old != new || first != second {
        return Err("来源验收改变了控制库身份、placement、保留表或其他表".into());
    }
    Ok(json!({"api_requests":{"login":11,"logout":11},
        "database_rows":{"sys_login_info":11,"sys_oper_log":0,"sys_outbox_event":0},
        "authorization_cache":{"created_and_removed_keys":33},
        "business_mutations":0,"object_mutations":0}))
}
