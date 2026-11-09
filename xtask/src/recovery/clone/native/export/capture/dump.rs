use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use regex::Regex;
use serde_json::{Value, json};

use super::super::super::{
    digest::FileDigest,
    input::{ArtifactBinding, Database},
    json as json_io, physical, schedule, sql,
    state::{self, Observation},
};
use super::super::database::Models;
use crate::Result;

pub(in crate::recovery::clone::native::export) struct DumpState {
    pub(in crate::recovery::clone::native::export) schedules: Vec<Value>,
    pub(in crate::recovery::clone::native::export) logical_tenants: BTreeSet<String>,
}

pub(in crate::recovery::clone::native::export) enum PhysicalRules {
    Capture,
    Frozen,
}

pub(in crate::recovery::clone::native::export) fn validate_dump_state(
    source: &Value,
    endpoint: &str,
    root: &Path,
    models: &Models,
    dumps: &[Value],
    objects: &BTreeMap<(String, String), Value>,
    rules: PhysicalRules,
) -> Result<DumpState> {
    let scope = dump_scope(source, endpoint, root, models, dumps)?;
    let frozen = match rules {
        PhysicalRules::Capture => None,
        PhysicalRules::Frozen => Some(physical::frozen::Check::new(
            &scope.forbidden,
            &scope.tenants,
        )?),
    };
    let (observed, schedules) = dump_rows(root, models, dumps, &scope, |row| match &frozen {
        Some(check) => check.row(row),
        None => capture_row_scope(row, &scope),
    })?;
    let object_bindings = object_bindings(objects)?;
    state::validate_relations(&observed, &scope.databases, &object_bindings)?;
    Ok(DumpState {
        schedules,
        logical_tenants: scope.tenants,
    })
}

fn capture_row_scope(row: &sql::Row, scope: &DumpScope) -> Result<()> {
    if let Some(value) = row.get("tenant_id").filter(|value| !value.is_null()) {
        let tenant = value.text().ok_or("数据引用未登记的逻辑租户")?;
        if !scope.tenants.contains(tenant) {
            return Err("数据引用未登记的逻辑租户".into());
        }
    }
    state::reject_physical(row, &scope.forbidden, &scope.tenants)
}

struct DumpScope {
    databases: BTreeMap<String, Database>,
    tenants: BTreeSet<String>,
    forbidden: Vec<String>,
}

fn dump_scope(
    source: &Value,
    endpoint: &str,
    root: &Path,
    models: &Models,
    dumps: &[Value],
) -> Result<DumpScope> {
    let databases = json_io::array(&source["databases"], "源数据库目标")?
        .iter()
        .map(|value| {
            Ok((
                value["key"].as_str().ok_or("数据库 key 无效")?.to_owned(),
                database_from(value)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let combined = databases
        .values()
        .find(|database| database.kind == "combined")
        .ok_or("必须恰有一个完整控制库")?;
    let control_dump = dumps
        .iter()
        .find(|dump| dump["key"].as_str() == Some(combined.key.as_str()))
        .ok_or("缺少控制库转储")?;
    let artifact = root.join(
        control_dump["artifact"]["file"]
            .as_str()
            .ok_or("控制库转储缺少文件名")?,
    );
    let mut tenants = BTreeSet::new();
    let tenant_name = Regex::new(r"^[A-Za-z0-9_-]{1,64}$")?;
    let catalog = sql::all_columns(&models.control, &models.tenant, true);
    sql::for_each_row(&artifact, &catalog, Some("sys_tenant"), |_, row| {
        let value = row
            .get("tenant_id")
            .and_then(sql::SqlValue::text)
            .ok_or("来源逻辑租户标识无效")?;
        if !tenant_name.is_match(value) || !tenants.insert(value.to_owned()) {
            return Err("来源逻辑租户标识无效或重复".into());
        }
        Ok(())
    })?;
    if !tenants.contains("system") {
        return Err("完整来源必须包含系统租户".into());
    }
    let mut forbidden = vec![
        json_io::text(&source["scope_id"], "来源 scope")?.to_owned(),
        endpoint.to_owned(),
    ];
    forbidden.extend(databases.values().map(|database| database.database.clone()));
    Ok(DumpScope {
        databases,
        tenants,
        forbidden,
    })
}

fn dump_rows(
    root: &Path,
    models: &Models,
    dumps: &[Value],
    scope: &DumpScope,
    physical: impl Fn(&sql::Row) -> Result<()>,
) -> Result<(Observation, Vec<Value>)> {
    let mut observed = Observation::default();
    let mut schedules = Vec::new();
    for dump in dumps {
        let database = scope
            .databases
            .get(dump["key"].as_str().unwrap_or_default())
            .ok_or("数据库转储逻辑目标未知")?;
        let catalog =
            sql::all_columns(&models.control, &models.tenant, database.kind == "combined");
        let file = root.join(
            dump["artifact"]["file"]
                .as_str()
                .ok_or("数据库转储缺少文件名")?,
        );
        let mut actual = sql::empty_counts(&catalog);
        sql::for_each_row(&file, &catalog, None, |table, row| {
            validate_row(table, row, &mut schedules, &physical)?;
            *actual.get_mut(table).ok_or("转储包含未知表")? += 1;
            state::collect(&mut observed, table, row, &database.key)
        })?;
        let declared = dump_tables(dump)?;
        if actual != declared {
            return Err("实际 SQL 转储不完整或行数与逻辑 inventory 不同".into());
        }
    }
    Ok((observed, schedules))
}

fn dump_tables(dump: &Value) -> Result<BTreeMap<String, u64>> {
    dump["tables"]
        .as_object()
        .ok_or("数据库转储缺少表行数")?
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                value.as_u64().ok_or("数据库转储行数必须为非负整数")?,
            ))
        })
        .collect()
}

fn validate_row(
    table: &str,
    row: &sql::Row,
    schedules: &mut Vec<Value>,
    physical: &impl Fn(&sql::Row) -> Result<()>,
) -> Result<()> {
    physical(row)?;
    if table == "sys_job_schedule" {
        let enabled = row
            .get("enabled")
            .and_then(sql::SqlValue::integer)
            .ok_or("调度 enabled 必须为整数")?;
        let deleted = row
            .get("del_flag")
            .and_then(sql::SqlValue::text)
            .ok_or("调度 del_flag 必须为字符串")?;
        if enabled == 1 && deleted == "0" {
            if row.get("tenant_id").and_then(sql::SqlValue::text) != Some("system") {
                return Err("非 system 启用调度没有既定目标处置边界".into());
            }
            let id = row
                .get("id")
                .and_then(sql::SqlValue::integer)
                .ok_or("调度 id 必须为整数")?;
            schedules.push(json!({
                "tenant_id": "system",
                "schedule_id": id.to_string(),
                "handler_key": row.get("handler_key").and_then(sql::SqlValue::text).ok_or("调度 handler 无效")?,
                "expected_version": row.get("version").and_then(sql::SqlValue::integer).ok_or("调度版本无效")?,
                "source_row_sha256": schedule::row_sha256(row)?,
            }));
        } else {
            state::validate_state(table, row)?;
        }
    } else {
        state::validate_state(table, row)?;
    }
    Ok(())
}

fn object_bindings(
    objects: &BTreeMap<(String, String), Value>,
) -> Result<BTreeMap<(String, String), ArtifactBinding>> {
    objects
        .iter()
        .map(|(key, value)| {
            Ok((
                key.clone(),
                ArtifactBinding {
                    file: String::new(),
                    digest: FileDigest {
                        bytes: value["bytes"].as_u64().ok_or("对象库存字节无效")?,
                        sha256: value["sha256"]
                            .as_str()
                            .ok_or("对象库存摘要无效")?
                            .to_owned(),
                    },
                },
            ))
        })
        .collect()
}

fn database_from(value: &Value) -> Result<Database> {
    Ok(Database {
        key: value["key"].as_str().ok_or("数据库 key 无效")?.to_owned(),
        kind: value["kind"].as_str().ok_or("数据库 kind 无效")?.to_owned(),
        mode: value["mode"].as_str().ok_or("数据库 mode 无效")?.to_owned(),
        server_uuid: value["server_uuid"]
            .as_str()
            .ok_or("数据库 server_uuid 无效")?
            .to_owned(),
        database: value["database"]
            .as_str()
            .ok_or("数据库名称无效")?
            .to_owned(),
        schema_sha256: value["schema_sha256"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    })
}
