use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use regex::Regex;
use serde_json::{Value, json};

use super::super::{digest, json as json_io, json::exact};
use super::{
    database::{self, Models},
    input::Context,
    inventory_validate,
    transport::Transport,
};
use crate::Result;

pub(super) const EXCLUDED: &[&str] = &[
    "ryframe_resource_ownership",
    "sys_backup_set",
    "sys_backup_resource",
    "sys_restore_run",
];

pub(crate) fn capture<T: Transport>(
    context: &Context,
    transport: &T,
    models: &Models,
    phase: &str,
    quiesced_at: &str,
    observed: bool,
) -> Result<Value> {
    let output = context.output();
    let runtime_file = output.join(format!("inventory-{phase}-runtime.json"));
    let executable = context.maintenance_executable("tenant-data")?;
    let sha = generation_head(context)?;
    let mut args = vec![
        "backup-inventory".into(),
        "--output".into(),
        runtime_file.display().to_string(),
        "--source-sha".into(),
        sha.clone(),
        if observed {
            "--observed-at".into()
        } else {
            "--quiesced-at".into()
        },
        quiesced_at.into(),
    ];
    run(
        context,
        transport,
        &executable,
        &args,
        &format!("inventory-{phase}"),
    )?;
    let runtime = json_io::read(&runtime_file)?;
    let mut target_values = Vec::new();
    let mut target_bindings = Vec::new();
    for database in context.databases()? {
        let key = database["key"].as_str().ok_or("数据库 key 无效")?;
        let path = output.join(format!("inventory-{phase}-target-{key}.json"));
        args = vec![
            "target-inventory".into(),
            "--target".into(),
            key.into(),
            "--output".into(),
            path.display().to_string(),
        ];
        run(
            context,
            transport,
            &executable,
            &args,
            &format!("inventory-{phase}-target-{key}"),
        )?;
        target_values.push(json_io::read(&path)?);
        target_bindings.push(json!({
            "file": path.file_name().and_then(|value| value.to_str()).ok_or("库存文件名必须为 UTF-8")?,
            "bytes": digest::file(&path)?.bytes,
            "sha256": digest::file(&path)?.sha256,
        }));
    }
    let value = complete(&runtime, target_values, context)?;
    validate(&value, context, models, &sha, quiesced_at, observed)?;
    json_io::write_new(
        &output.join(format!("inventory-{phase}-targets.json")),
        &json!({"runtime": {
            "file": runtime_file.file_name().and_then(|value| value.to_str()).ok_or("库存文件名必须为 UTF-8")?,
            "bytes": digest::file(&runtime_file)?.bytes,
            "sha256": digest::file(&runtime_file)?.sha256,
        }, "targets": target_bindings}),
    )?;
    json_io::write_new(&output.join(format!("inventory-{phase}.json")), &value)?;
    Ok(value)
}

pub(crate) fn logical(value: &Value) -> Result<Value> {
    let databases = value
        .get("databases")
        .and_then(Value::as_array)
        .ok_or("库存缺少数据库")?
        .iter()
        .map(|database| {
            let mut database = database.clone();
            if let Some(tables) = database.get_mut("tables").and_then(Value::as_array_mut) {
                tables.sort_by_key(|item| {
                    item.get("table")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned()
                });
            }
            if let Some(placements) = database.get_mut("placements").and_then(Value::as_array_mut) {
                placements.sort_by_key(|item| {
                    item.get("tenant_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned()
                });
            }
            database
        })
        .collect::<Vec<_>>();
    let objects = value
        .get("objects")
        .and_then(Value::as_array)
        .ok_or("库存缺少对象")?
        .iter()
        .map(|bucket| {
            let mut bucket = bucket.clone();
            if let Some(entries) = bucket.get_mut("entries").and_then(Value::as_array_mut) {
                entries.sort_by_key(|item| {
                    item.get("key")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned()
                });
            }
            bucket
        })
        .collect::<Vec<_>>();
    let mut result = value.as_object().cloned().ok_or("库存必须为对象")?;
    result.remove("captured_at");
    result.insert("databases".into(), Value::Array(databases));
    result.insert("objects".into(), Value::Array(objects));
    Ok(Value::Object(result))
}

pub(crate) fn object_index(value: &Value) -> Result<BTreeMap<(String, String), Value>> {
    let mut result = BTreeMap::new();
    for bucket in value
        .get("objects")
        .and_then(Value::as_array)
        .ok_or("库存缺少对象")?
    {
        let name = bucket
            .get("bucket")
            .and_then(Value::as_str)
            .ok_or("库存对象桶无效")?;
        let prefix = bucket
            .get("prefix")
            .and_then(Value::as_str)
            .ok_or("库存对象桶缺少前缀")?;
        for entry in bucket
            .get("entries")
            .and_then(Value::as_array)
            .ok_or("库存对象条目无效")?
        {
            let key = entry
                .get("key")
                .and_then(Value::as_str)
                .ok_or("库存对象 key 无效")?;
            result.insert(
                (
                    name.to_owned(),
                    key.strip_prefix(prefix).unwrap_or(key).to_owned(),
                ),
                json!({
                    "bytes": entry["bytes"],
                    "sha256": entry["sha256"],
                }),
            );
        }
    }
    Ok(result)
}

fn complete(runtime: &Value, targets: Vec<Value>, context: &Context) -> Result<Value> {
    complete_selected(runtime, targets, context.source())
}

/// 合成运行中观察和停机库存共用的四目标事实，不改变两类时间字段的含义。
pub(crate) fn complete_selected(
    runtime: &Value,
    targets: Vec<Value>,
    selected: &Value,
) -> Result<Value> {
    let declared = json_io::array(&selected["databases"], "来源数据库")?
        .iter()
        .map(|item| item["key"].as_str().unwrap_or_default().to_owned())
        .collect::<BTreeSet<_>>();
    let time_field = if runtime.get("observed_at").is_some() {
        "observed_at"
    } else {
        "quiesced_at"
    };
    let fields = [
        "scope_id",
        "source_sha",
        time_field,
        "captured_at",
        "control_schema_fingerprint",
        "tenant_schema_fingerprint",
        "databases",
        "objects",
    ];
    let runtime = json_io::exact(runtime, &fields, "常规库存")?;
    let by_key = collect_targets(runtime, targets, &declared)?;
    let mut seen = BTreeSet::new();
    check_runtime_databases(runtime, &by_key, &mut seen)?;
    check_empty_targets(&declared, &by_key, &seen)?;
    let mut value = json!({
        "scope_id": runtime["scope_id"],
        "source_sha": runtime["source_sha"],
        "captured_at": runtime["captured_at"],
        "control_schema_fingerprint": runtime["control_schema_fingerprint"],
        "tenant_schema_fingerprint": runtime["tenant_schema_fingerprint"],
        "databases": declared.into_iter().map(|key| by_key.get(&key).unwrap().clone()).collect::<Vec<_>>(),
        "objects": runtime["objects"],
    });
    value[time_field] = runtime[time_field].clone();
    Ok(value)
}

fn collect_targets(
    runtime: &serde_json::Map<String, Value>,
    targets: Vec<Value>,
    declared: &BTreeSet<String>,
) -> Result<BTreeMap<String, Value>> {
    let mut by_key = BTreeMap::new();
    for target in targets {
        let target = exact(
            &target,
            &[
                "scope_id",
                "control_schema_fingerprint",
                "tenant_schema_fingerprint",
                "target",
            ],
            "逐目标库存",
        )?;
        for field in [
            "scope_id",
            "control_schema_fingerprint",
            "tenant_schema_fingerprint",
        ] {
            if target.get(field) != runtime.get(field) {
                return Err("逐目标库存的 scope 或 schema 与常规库存不同".into());
            }
        }
        let target_root = exact(
            target.get("target").unwrap(),
            &["database", "preserved_tables"],
            "逐目标库存",
        )?;
        let database = target_root
            .get("database")
            .ok_or("逐目标库存缺少数据库")?
            .clone();
        let key = database
            .get("key")
            .and_then(Value::as_str)
            .ok_or("逐目标库存数据库 key 无效")?;
        if !declared.contains(key) || by_key.insert(key.to_owned(), database).is_some() {
            return Err("逐目标库存含未登记或重复目标".into());
        }
    }
    if by_key.keys().cloned().collect::<BTreeSet<_>>() != *declared {
        return Err("逐目标库存未覆盖全部登记目标".into());
    }
    Ok(by_key)
}

fn check_runtime_databases(
    runtime: &serde_json::Map<String, Value>,
    by_key: &BTreeMap<String, Value>,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    for database in runtime
        .get("databases")
        .and_then(Value::as_array)
        .ok_or("常规库存缺少数据库")?
    {
        let key = database
            .get("key")
            .and_then(Value::as_str)
            .ok_or("常规库存数据库 key 无效")?;
        let target = by_key.get(key).ok_or("常规库存包含未登记目标")?;
        if database_image(database) != database_image(target) || !seen.insert(key.to_owned()) {
            return Err("常规库存与逐目标实际库存不一致".into());
        }
    }
    Ok(())
}

fn check_empty_targets(
    declared: &BTreeSet<String>,
    by_key: &BTreeMap<String, Value>,
    seen: &BTreeSet<String>,
) -> Result<()> {
    for key in declared.difference(seen) {
        let database = by_key.get(key).ok_or("逐目标库存缺失")?;
        let placements = database
            .get("placements")
            .and_then(Value::as_array)
            .ok_or("逐目标库存缺少 placement")?;
        let tables = database
            .get("tables")
            .and_then(Value::as_array)
            .ok_or("逐目标库存缺少表")?;
        if !placements.is_empty()
            || tables.iter().any(|table| {
                table.get("rows").and_then(Value::as_u64)
                    != Some(u64::from(
                        table.get("table").and_then(Value::as_str)
                            == Some("biz_tenant_target_slot"),
                    ))
            })
        {
            return Err("常规库存遗漏了非空目标，不能补作空目标".into());
        }
    }
    Ok(())
}

fn database_image(value: &Value) -> Value {
    let mut value = value.clone();
    if let Some(tables) = value.get_mut("tables").and_then(Value::as_array_mut) {
        tables.sort_by_key(|item| {
            item.get("table")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        });
    }
    if let Some(placements) = value.get_mut("placements").and_then(Value::as_array_mut) {
        placements.sort_by_key(|item| {
            item.get("tenant_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        });
    }
    value
}

fn validate(
    value: &Value,
    context: &Context,
    models: &Models,
    source_sha: &str,
    quiesced_at: &str,
    observed: bool,
) -> Result<()> {
    validate_selected(
        value,
        context.source(),
        models,
        source_sha,
        quiesced_at,
        observed,
    )
}

/// 来源运行完整像复用同一库存模型，不借用停机导出的执行资格。
pub(crate) fn validate_selected(
    value: &Value,
    selected: &Value,
    models: &Models,
    source_sha: &str,
    quiesced_at: &str,
    observed: bool,
) -> Result<()> {
    let time_field = if observed {
        "observed_at"
    } else {
        "quiesced_at"
    };
    let fields = [
        "scope_id",
        "source_sha",
        time_field,
        "captured_at",
        "control_schema_fingerprint",
        "tenant_schema_fingerprint",
        "databases",
        "objects",
    ];
    let root = exact(value, &fields, "源库存")?;
    let scope = selected["scope_id"].as_str().ok_or("源 scope 无效")?;
    inventory_validate::header(root, scope, source_sha, quiesced_at, observed)?;
    let declared_values = json_io::array(&selected["databases"], "来源数据库")?;
    let declared = declared_values
        .iter()
        .map(|item| {
            (
                item["key"].as_str().unwrap_or_default().to_owned(),
                item.clone(),
            )
        })
        .collect::<BTreeMap<String, Value>>();
    inventory_validate::databases(root, &declared, models)?;
    inventory_validate::objects(root, scope)
}

fn run<T: Transport>(
    context: &Context,
    transport: &T,
    executable: &Path,
    args: &[String],
    label: &str,
) -> Result<()> {
    database::run_logged(context, transport, label, executable, args, None)
}

fn generation_head(context: &Context) -> Result<String> {
    let path = context
        .request
        .get("backend_build")
        .and_then(|value| value.get("path"))
        .and_then(Value::as_str)
        .ok_or("源请求缺少 API/Worker 构建路径")?;
    let value = json_io::read(Path::new(path))?;
    value
        .pointer("/sources/full/source/snapshot/head")
        .and_then(Value::as_str)
        .filter(|value| Regex::new(r"^[a-f0-9]{40}$").is_ok_and(|re| re.is_match(value)))
        .map(ToOwned::to_owned)
        .ok_or_else(|| "API/Worker 构建缺少精确源码 HEAD".into())
}
