use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::{Map, Value, json};

use super::{
    catalog::{self, Catalog},
    digest,
    input::{self, ArtifactBinding, ValidatedInput},
    json::{array, exact, object, text},
    schedule, sql,
    state::{self, Observation},
};
use crate::Result;

pub(crate) struct Output {
    pub(crate) value: Value,
    pub(crate) sources: Vec<PathBuf>,
}

pub(crate) fn create(value: &Value, backend: &Path) -> Result<Output> {
    let validated = input::validate(value, backend)?;
    let policy_before = source_policy(&validated.sources)?;
    let (databases, pending) = database_plan(value, &validated)?;
    rebind_all(value, &validated.artifact_root)?;
    if source_policy(&validated.sources)? != policy_before {
        return Err("离线核验期间 schema、生成目录或业务目录发生变化".into());
    }
    let status = if pending.is_empty() {
        "offline_verified"
    } else {
        "offline_verified_pending_target_actions"
    };
    let mut result = Map::new();
    result.insert("format_version".into(), json!(1));
    result.insert(
        "kind".into(),
        Value::String("devex-clone-offline-plan".into()),
    );
    result.insert("copy_id".into(), value["copy_id"].clone());
    result.insert("copy_stage".into(), value["copy_stage"].clone());
    result.insert(
        "input_sha256".into(),
        Value::String(digest::plan_hash(value)?),
    );
    result.insert("source_snapshot".into(), value["source_snapshot"].clone());
    result.insert("source_scope".into(), value["source"]["scope_id"].clone());
    result.insert("target_scope".into(), value["target"]["scope_id"].clone());
    result.insert(
        "catalog_sha256".into(),
        Value::String(digest::plan_hash(&json!({
            "control": validated.control,
            "tenant": validated.tenant,
        }))?),
    );
    result.insert("databases".into(), Value::Array(databases));
    result.insert("objects".into(), Value::Array(validated.objects.entries));
    result.insert("evidence".into(), value["evidence"].clone());
    result.insert("excluded_tables".into(), json!(catalog::excluded_tables()));
    result.insert("status".into(), Value::String(status.into()));
    result.insert("pending_target_actions".into(), Value::Array(pending));
    result.insert("target_ready".into(), Value::Bool(false));
    result.insert("worker_must_remain_stopped".into(), Value::Bool(true));
    result.insert("execution_authorized".into(), Value::Bool(false));
    result.insert(
        "live_preconditions".into(),
        Value::String(
            "离线仅绑定已提供证据字节；执行前仍须重新验证停止状态、新目标创建事实、UUID、ownership、完整schema及源未变化；该计划本身不授予写入权限"
                .into(),
        ),
    );
    let result = Value::Object(result);
    let mut wrapped = result.as_object().unwrap().clone();
    wrapped.insert(
        "plan_sha256".into(),
        Value::String(digest::plan_hash(&result)?),
    );
    Ok(Output {
        value: Value::Object(wrapped),
        sources: validated.sources,
    })
}

fn source_policy(sources: &[PathBuf]) -> Result<BTreeMap<String, String>> {
    let mut paths = sources.to_vec();
    paths.sort();
    paths.dedup();
    paths
        .iter()
        .map(|path| {
            Ok((
                path.to_string_lossy().into_owned(),
                digest::canonical_binding(&digest::file(path)?),
            ))
        })
        .collect()
}

fn database_plan(value: &Value, input: &ValidatedInput) -> Result<(Vec<Value>, Vec<Value>)> {
    let dumps = array(
        value.get("databases").ok_or("复制输入缺少 databases")?,
        "数据库转储",
    )?;
    if dumps.len() != input.old.len() {
        return Err("必须完整列出每个数据库的数据转储".into());
    }
    let mut actions = schedule::actions(value)?;
    let logical = declared_tenants(value, input)?;
    let forbidden = forbidden(value, input);
    let mut observed = Observation::default();
    let mut seen = BTreeSet::new();
    let mut databases = Vec::with_capacity(dumps.len());
    let mut pending = Vec::new();
    for dump in dumps {
        let dump = exact(dump, &["key", "tables", "artifact"], "数据库转储")?;
        let key = text(dump.get("key").unwrap(), "数据库转储键")?;
        let old = input.old.get(key).ok_or("数据转储逻辑目标重复或未知")?;
        if !seen.insert(key.to_owned()) {
            return Err("数据转储逻辑目标重复或未知".into());
        }
        let catalog = sql::all_columns(&input.control, &input.tenant, old.kind == "combined");
        let declared = exact_table_counts(dump.get("tables").unwrap(), &catalog)?;
        let binding = input::bound_file(&input.artifact_root, dump.get("artifact").unwrap())?;
        let mut counts = sql::empty_counts(&catalog);
        sql::for_each_row(
            &input.artifact_root.join(&binding.file),
            &catalog,
            None,
            |table, row| {
                if let Some(value) = row.get("tenant_id").filter(|value| !value.is_null()) {
                    let tenant = value.text().ok_or("数据引用未登记的逻辑租户")?;
                    if !logical.contains(tenant) {
                        return Err("数据引用未登记的逻辑租户".into());
                    }
                }
                state::reject_physical(row, &forbidden, &logical)?;
                if table == "sys_job_schedule" {
                    if let Some(mut action) = schedule::inspect(row, &mut actions)? {
                        action.as_object_mut().unwrap().insert(
                            "target_scope_id".into(),
                            value["target"]["scope_id"].clone(),
                        );
                        pending.push(action);
                    }
                } else {
                    state::validate_state(table, row)?;
                }
                *counts.get_mut(table).unwrap() += 1;
                state::collect(&mut observed, table, row, key)
            },
        )?;
        if counts != declared {
            return Err("导出实际行数不符合完整表清单".into());
        }
        let target = input.new.get(key).ok_or("目标缺少对应逻辑数据库")?;
        databases.push(json!({
            "key": key,
            "source_database": old.database,
            "target_database": target.database,
            "tables": counts,
            "artifact": binding_value(&binding),
            "preserve_target_ownership": true,
        }));
    }
    state::validate_relations(&observed, &input.old, &input.objects.index)?;
    if !actions.is_empty() {
        return Err("目标调度处置清单包含未命中的启用源记录".into());
    }
    Ok((databases, pending))
}

fn declared_tenants(value: &Value, input: &ValidatedInput) -> Result<BTreeSet<String>> {
    let combined = input
        .old
        .values()
        .find(|database| database.kind == "combined")
        .ok_or("必须恰有一个完整控制库")?;
    let dumps = array(value.get("databases").unwrap(), "数据库转储")?;
    let matches = dumps
        .iter()
        .filter(|dump| dump.get("key").and_then(Value::as_str) == Some(combined.key.as_str()))
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err("必须恰有一个完整控制库转储".into());
    }
    let binding = input::bound_file(&input.artifact_root, matches[0].get("artifact").unwrap())?;
    let ambiguous = std::iter::once(value["source"]["scope_id"].as_str().unwrap().to_owned())
        .chain(input.old.values().map(|database| database.database.clone()))
        .collect::<BTreeSet<_>>();
    let pattern = Regex::new(r"^[A-Za-z0-9_-]{1,64}$")?;
    let mut result = BTreeSet::new();
    sql::for_each_row(
        &input.artifact_root.join(&binding.file),
        &sql::all_columns(&input.control, &input.tenant, true),
        Some("sys_tenant"),
        |_, row| {
            let tenant = row
                .get("tenant_id")
                .and_then(sql::SqlValue::text)
                .ok_or("来源逻辑租户标识无效或重复")?;
            if !pattern.is_match(tenant)
                || ambiguous.contains(tenant)
                || !result.insert(tenant.to_owned())
            {
                return Err("来源逻辑租户标识无效或重复".into());
            }
            Ok(())
        },
    )?;
    if !result.contains("system") {
        return Err("完整来源必须包含系统租户".into());
    }
    Ok(result)
}

fn exact_table_counts(value: &Value, catalog: &Catalog) -> Result<BTreeMap<String, u64>> {
    let counts = object(value, "数据库表行数")?;
    if counts.keys().cloned().collect::<BTreeSet<_>>()
        != catalog.keys().cloned().collect::<BTreeSet<_>>()
    {
        return Err("白名单必须完整覆盖当前业务表，排除 owner、迁移和备份恢复元数据".into());
    }
    counts
        .iter()
        .map(|(table, count)| {
            let count = count.as_u64().ok_or("表行数必须明确且非负")?;
            Ok((table.clone(), count))
        })
        .collect()
}

fn forbidden(value: &Value, input: &ValidatedInput) -> Vec<String> {
    let mut result = vec![
        value["source"]["scope_id"].as_str().unwrap().to_owned(),
        value["source"]["object_endpoint"]
            .as_str()
            .unwrap()
            .to_owned(),
    ];
    result.extend(input.old.values().map(|database| database.database.clone()));
    result
}

fn rebind_all(value: &Value, root: &Path) -> Result<()> {
    for binding in value["evidence"]
        .as_object()
        .unwrap()
        .values()
        .chain(
            value["databases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| &item["artifact"]),
        )
        .chain(
            value["objects"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|bucket| bucket["entries"].as_array().unwrap())
                .map(|item| &item["artifact"]),
        )
    {
        input::bound_file(root, binding)?;
    }
    Ok(())
}

fn binding_value(binding: &ArtifactBinding) -> Value {
    json!({
        "file": binding.file,
        "bytes": binding.digest.bytes,
        "sha256": binding.digest.sha256,
    })
}
