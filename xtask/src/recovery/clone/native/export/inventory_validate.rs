use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde_json::Value;

use super::super::json::{exact, text};
use super::database::Models;
use super::inventory::EXCLUDED;
use crate::{
    Result,
    utc_timestamp::{now_order_key, rfc3339_order_key},
};

pub(super) fn header(
    root: &serde_json::Map<String, Value>,
    scope: &str,
    source_sha: &str,
    quiesced_at: &str,
    observed: bool,
) -> Result<()> {
    let time = root
        .get(if observed {
            "observed_at"
        } else {
            "quiesced_at"
        })
        .and_then(Value::as_str)
        .ok_or("源库存缺少观察时间")?;
    if root.get("scope_id").and_then(Value::as_str) != Some(scope)
        || root.get("source_sha").and_then(Value::as_str) != Some(source_sha)
        || rfc3339_order_key(time)? != rfc3339_order_key(quiesced_at)?
    {
        return Err("实际 inventory 与源 scope、构建或明确观察类型不同".into());
    }
    let captured = rfc3339_order_key(
        root.get("captured_at")
            .and_then(Value::as_str)
            .ok_or("源库存缺少采集时间")?,
    )?;
    if rfc3339_order_key(quiesced_at)? > captured || captured > now_order_key() {
        return Err("实际 inventory 采集时间与观察顺序不符".into());
    }
    let fingerprint_value = serde_json::json!({
        "control_schema_fingerprint": root["control_schema_fingerprint"],
        "tenant_schema_fingerprint": root["tenant_schema_fingerprint"],
    });
    let fingerprints = exact(
        &fingerprint_value,
        &["control_schema_fingerprint", "tenant_schema_fingerprint"],
        "schema 指纹",
    )?;
    if !Regex::new(r"^[a-f0-9]{16}$")?.is_match(text(
        fingerprints.get("control_schema_fingerprint").unwrap(),
        "控制 schema 指纹",
    )?) || !Regex::new(r"^[a-f0-9]{64}$")?.is_match(text(
        fingerprints.get("tenant_schema_fingerprint").unwrap(),
        "租户 schema 指纹",
    )?) {
        return Err("编译 schema 指纹格式与当前控制库或租户目录契约不同".into());
    }
    Ok(())
}

pub(super) fn databases(
    root: &serde_json::Map<String, Value>,
    declared: &BTreeMap<String, Value>,
    models: &Models,
) -> Result<()> {
    let databases = root
        .get("databases")
        .and_then(Value::as_array)
        .ok_or("inventory 缺少 databases")?;
    if databases.len() != declared.len()
        || databases
            .iter()
            .filter_map(|item| item.get("key").and_then(Value::as_str))
            .collect::<BTreeSet<_>>()
            != declared.keys().map(String::as_str).collect()
    {
        return Err("inventory 必须完整覆盖每个登记源目标，包括空目标；不能代填摘要".into());
    }
    let sha256 = Regex::new(r"^[a-f0-9]{64}$")?;
    for database in databases {
        database_entry(database, declared, models, &sha256)?;
    }
    Ok(())
}

fn database_entry(
    value: &Value,
    declared: &BTreeMap<String, Value>,
    models: &Models,
    sha256: &Regex,
) -> Result<()> {
    let database = exact(
        value,
        &[
            "key",
            "kind",
            "server_uuid",
            "database",
            "shared",
            "placements",
            "tables",
        ],
        "inventory 数据库",
    )?;
    let key = text(database.get("key").unwrap(), "数据库 key")?;
    let known = declared.get(key).ok_or("inventory 含未登记数据库")?;
    if database.get("kind") != known.get("kind")
        || database.get("server_uuid") != known.get("server_uuid")
        || database.get("database") != known.get("database")
        || database.get("shared").and_then(Value::as_bool)
            != Some(known.get("mode").and_then(Value::as_str) == Some("shared"))
    {
        return Err("inventory 数据库 UUID、名称、类型或共享模式与源计划不同".into());
    }
    let expected = expected_tables(database, models);
    tables(database, &expected, sha256)?;
    placements(database)
}

fn expected_tables(database: &serde_json::Map<String, Value>, models: &Models) -> BTreeSet<String> {
    if database["kind"].as_str() == Some("combined") {
        models
            .full_control
            .keys()
            .filter(|table| !EXCLUDED.contains(&table.as_str()))
            .chain(models.tenant.keys())
            .cloned()
            .collect()
    } else {
        models.tenant.keys().cloned().collect()
    }
}

fn tables(
    database: &serde_json::Map<String, Value>,
    expected: &BTreeSet<String>,
    sha256: &Regex,
) -> Result<()> {
    let tables = database
        .get("tables")
        .and_then(Value::as_array)
        .ok_or("inventory 缺少表清单")?;
    if tables.len() != expected.len()
        || tables
            .iter()
            .filter_map(|item| item.get("table").and_then(Value::as_str))
            .collect::<BTreeSet<_>>()
            != expected.iter().map(String::as_str).collect()
    {
        return Err("inventory 表清单不完整或包含未知表，空表也必须保留".into());
    }
    for table in tables {
        let table = exact(table, &["table", "rows", "sha256"], "inventory 表")?;
        if table.get("rows").and_then(Value::as_u64).is_none()
            || !sha256.is_match(text(table.get("sha256").unwrap(), "表摘要")?)
        {
            return Err("inventory 表行数或摘要无效".into());
        }
    }
    Ok(())
}

fn placements(database: &serde_json::Map<String, Value>) -> Result<()> {
    let mut tenants = BTreeSet::new();
    for placement in database
        .get("placements")
        .and_then(Value::as_array)
        .ok_or("inventory 缺少 placement")?
    {
        let placement = exact(
            placement,
            &["tenant_id", "generation", "switch_token"],
            "inventory placement",
        )?;
        let tenant = text(placement.get("tenant_id").unwrap(), "tenant_id")?;
        if tenant.is_empty()
            || !tenants.insert(tenant)
            || placement
                .get("generation")
                .and_then(Value::as_u64)
                .is_none_or(|value| value == 0)
            || placement
                .get("switch_token")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err("inventory placement 重复或代次不完整".into());
        }
    }
    Ok(())
}

pub(super) fn objects(root: &serde_json::Map<String, Value>, scope: &str) -> Result<()> {
    let objects = root
        .get("objects")
        .and_then(Value::as_array)
        .ok_or("inventory 缺少 objects")?;
    let buckets = super::super::input::BUCKETS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if objects.len() != buckets.len()
        || objects
            .iter()
            .filter_map(|item| item.get("bucket").and_then(Value::as_str))
            .collect::<BTreeSet<_>>()
            != buckets
    {
        return Err("inventory 必须完整覆盖五个固定桶".into());
    }
    let sha256 = Regex::new(r"^[a-f0-9]{64}$")?;
    for bucket in objects {
        object_bucket(bucket, scope, &sha256)?;
    }
    Ok(())
}

fn object_bucket(value: &Value, scope: &str, sha256: &Regex) -> Result<()> {
    let bucket = exact(value, &["bucket", "prefix", "entries"], "inventory 对象桶")?;
    let prefix = text(bucket.get("prefix").unwrap(), "对象前缀")?;
    if prefix != format!("{scope}/") {
        return Err("inventory 对象范围必须是源精确 scope 前缀".into());
    }
    let mut keys = BTreeSet::new();
    for entry in bucket
        .get("entries")
        .and_then(Value::as_array)
        .ok_or("inventory 对象条目无效")?
    {
        let entry = exact(entry, &["key", "bytes", "sha256"], "inventory 对象")?;
        let key = text(entry.get("key").unwrap(), "对象 key")?;
        if !keys.insert(key)
            || !key.starts_with(prefix)
            || key == format!("{prefix}.ryframe-owner")
            || entry.get("bytes").and_then(Value::as_u64).is_none()
            || !sha256.is_match(text(entry.get("sha256").unwrap(), "对象摘要")?)
        {
            return Err("inventory 对象重复、越界或超过明确大小上限".into());
        }
    }
    Ok(())
}
