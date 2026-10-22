use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use chrono::{SecondsFormat, Utc};
use regex::Regex;
use serde_json::{Value, json};

use super::super::{
    digest::{self, FileDigest},
    input::{ArtifactBinding, Database},
    json as json_io,
    resources::storage::registered,
    schedule, sql,
    state::{self, Observation},
};
use super::{
    database::{self, Models},
    input::Context,
    inventory, object, proof, recovery,
    transport::SystemTransport,
    verify,
};
use crate::{Result, cli::CloneSide};

pub(crate) fn run(context: &Context) -> Result<Value> {
    if context.source_export().is_some() {
        if context.mode != "run" {
            return Err("固定清单已有外部 export，无需采用或恢复 attempt 候选".into());
        }
        let exported = context.source_export().unwrap().clone();
        let verified = verify::binding(context, &exported)?;
        return Ok(json!({
            "status": "export_verified",
            "export": exported,
            "logical_inventory_sha256": verified["logical_inventory_sha256"],
            "remote_writes": 0,
            "restore_qualified": false,
        }));
    }
    recovery::reject_reexport(&context.backend, &context.run_dir, context.attempt)?;
    let output = context.output();
    recovery::prepare_intent(context)?;
    fs::create_dir(&output)
        .map_err(|error| format!("无法创建源导出证据目录 {}：{error}", output.display()))?;
    let transport = SystemTransport;
    match capture(context, &transport) {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = json_io::write_new(
                &output.join("failure.json"),
                &json!({
                    "format_version": 1,
                    "status": "source_export_failed",
                    "stage": "source_export",
                    "error_type": "StageError",
                    "reason": error.to_string(),
                    "clone_verified": false,
                    "restore_qualified": false,
                }),
            );
            Err(error)
        }
    }
}

struct CaptureFrame {
    before: Value,
    models: Models,
    schema_before: Vec<Value>,
    quiesced: String,
    inventory_before: Value,
    dumps: Vec<Value>,
    schedules: Vec<Value>,
    objects: Vec<Value>,
}

fn capture(context: &Context, transport: &SystemTransport) -> Result<Value> {
    let storage_before =
        registered::current(&context.backend, &context.run_dir, CloneSide::Source)?;
    let frame = capture_before(context, transport)?;
    capture_after(context, transport, &frame)?;
    if registered::current(&context.backend, &context.run_dir, CloneSide::Source)? != storage_before
    {
        return Err("源 storage provenance 在导出前后发生变化".into());
    }
    publish_export(context, frame)
}

fn capture_before(context: &Context, transport: &SystemTransport) -> Result<CaptureFrame> {
    let output = context.output();
    json_io::write_new(
        &output.join("request.json"),
        &super::input::public_request(&context.request),
    )?;
    let before = proof::generation(context)?;
    json_io::write_new(&output.join("generation-before.json"), &before)?;
    let models = database::models(context)?;
    verify_databases(context, transport, &models)?;
    object::verify_owners(context, transport, "owner-before")?;
    database::verify_migrations(context, transport, "before")?;
    let schema_before = database::schema_snapshot(context, &models, transport, "before")?;
    json_io::write_new(
        &output.join("schema-before.json"),
        &json!({"databases": schema_before}),
    )?;
    ensure_generation(context, &before)?;
    let quiesced = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false);
    let inventory_before =
        inventory::capture(context, transport, &models, "before", &quiesced, false)?;
    let before_index = inventory::object_index(&inventory_before)?;
    let dumps = capture_dumps(context, transport, &models)?;
    ensure_generation(context, &before)?;
    let schedules_before = validate_dump_state(context, &models, &dumps, &before_index)?;
    let (objects, object_index) = object::capture(
        context,
        transport,
        &inventory_before,
        context.request["max_object_bytes"]
            .as_u64()
            .ok_or("源导出缺少对象大小上限")?,
    )?;
    let schedules = validate_dump_state(context, &models, &dumps, &object_index)?;
    if schedules != schedules_before {
        return Err("源调度行在对象抓取前后变化，拒绝不同 SQL 快照".into());
    }
    Ok(CaptureFrame {
        before,
        models,
        schema_before,
        quiesced,
        inventory_before,
        dumps,
        schedules,
        objects,
    })
}

fn capture_dumps(
    context: &Context,
    transport: &SystemTransport,
    models: &Models,
) -> Result<Vec<Value>> {
    let mut dumps = Vec::new();
    for database in context.databases()? {
        let (counts, artifact) = database::dump(context, transport, &database, models)?;
        dumps.push(json!({
            "key": database["key"],
            "tables": counts,
            "artifact": artifact,
        }));
    }
    Ok(dumps)
}

fn capture_after(
    context: &Context,
    transport: &SystemTransport,
    frame: &CaptureFrame,
) -> Result<()> {
    ensure_generation(context, &frame.before)?;
    let inventory_after = inventory::capture(
        context,
        transport,
        &frame.models,
        "after",
        &frame.quiesced,
        false,
    )?;
    if inventory::logical(&frame.inventory_before)? != inventory::logical(&inventory_after)? {
        return Err("源前后逻辑表摘要、placement 或对象清单变化，拒绝拼接不同快照".into());
    }
    verify_databases(context, transport, &frame.models)?;
    object::verify_owners(context, transport, "owner-after")?;
    database::verify_migrations(context, transport, "after")?;
    let schema_after = database::schema_snapshot(context, &frame.models, transport, "after")?;
    json_io::write_new(
        &context.output().join("schema-after.json"),
        &json!({"databases": schema_after}),
    )?;
    if frame.schema_before != schema_after {
        return Err("源 schema 在导出期间变化".into());
    }
    let after = proof::generation(context)?;
    json_io::write_new(&context.output().join("generation-after.json"), &after)?;
    if frame.before != after {
        return Err("源导出期间源码、构建、配置、producer 或工具代次变化".into());
    }
    Ok(())
}

fn ensure_generation(context: &Context, expected: &Value) -> Result<()> {
    if proof::generation(context)? != *expected {
        return Err("源导出期间源码、构建、配置、producer 或工具代次变化".into());
    }
    Ok(())
}

fn publish_export(context: &Context, frame: CaptureFrame) -> Result<Value> {
    let output = context.output();
    verify_files(&output, &frame.dumps, &frame.objects)?;
    let request_digest = digest::file(&context.request_path)?;
    let export = build_export(context, &frame, request_digest)?;
    json_io::write_new(&output.join("export.json"), &export)?;
    let binding = verify::binding(
        context,
        &json!({
            "path": output.join("export.json").display().to_string(),
            "bytes": digest::file(&output.join("export.json"))?.bytes,
            "sha256": digest::file(&output.join("export.json"))?.sha256,
        }),
    )?;
    let verified = json!({
        "export": export,
        "binding": binding,
        "summary": {
            "export_sha256": digest::plan_hash(&export)?,
            "logical_inventory_sha256": export["logical_inventory_sha256"],
            "databases": export["databases"].as_array().map(Vec::len).unwrap_or(0),
            "objects": export["objects"].as_array().map(|items| items.iter().map(|bucket| bucket["entries"].as_array().map(Vec::len).unwrap_or(0)).sum::<usize>()).unwrap_or(0),
        },
    });
    recovery::record_verified(context, &verified)?;
    Ok(json!({
        "status": "export_verified",
        "export": {
            "path": output.join("export.json").display().to_string(),
            "bytes": digest::file(&output.join("export.json"))?.bytes,
            "sha256": digest::file(&output.join("export.json"))?.sha256,
        },
        "logical_inventory_sha256": export["logical_inventory_sha256"],
        "remote_writes": 0,
        "restore_qualified": false,
    }))
}

fn build_export(
    context: &Context,
    frame: &CaptureFrame,
    request_digest: digest::FileDigest,
) -> Result<Value> {
    let output = context.output();
    Ok(json!({
        "format_version": 1,
        "kind": "devex-clone-source-export",
        "status": "source_export_captured",
        "id": context.request["id"],
        "scope_id": context.request["source"]["scope_id"],
        "request": {
            "path": context.request_path.display().to_string(),
            "bytes": request_digest.bytes,
            "sha256": request_digest.sha256,
        },
        "source_snapshot": frame.before["source"],
        "worktree_fingerprint": context.request["worktree_fingerprint"],
        "source": super::input::public_request(&context.request)["source"],
        "artifact_root": output,
        "databases": frame.dumps,
        "objects": frame.objects,
        "enabled_system_schedule_rows": frame.schedules,
        "logical_inventory_sha256": digest::plan_hash(&inventory::logical(&frame.inventory_before)?)?,
        "evidence": evidence(&output)?,
        "catalog_sha256": digest::plan_hash(&json!({
            "control": frame.models.control,
            "tenant": frame.models.tenant,
        }))?,
        "quiesced_at": frame.quiesced,
        "completed_at": Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false),
        "requires_offline_plan": true,
        "operator_declared_producers_only": true,
        "remote_writes": 0,
        "clone_verified": false,
        "restore_qualified": false,
        "dump_semantic_digest_verified": false,
        "dump_validation": "固定外部工具与明确源连接、完整列、逐表行数、文件 SHA；实际目标导入后仍须匹配源 inventory 逻辑摘要",
    }))
}

fn verify_databases(
    context: &Context,
    transport: &SystemTransport,
    _models: &Models,
) -> Result<()> {
    let scope = context.source()["scope_id"]
        .as_str()
        .ok_or("源 scope 无效")?;
    for database in context.databases()? {
        let key = database["key"].as_str().ok_or("数据库 key 无效")?;
        let response = database::mysql(
            context,
            transport,
            &database,
            "SELECT @@server_uuid, DATABASE();",
            "数据库 identity 核验",
        )?;
        if response.split('\t').collect::<Vec<_>>()
            != vec![
                database["server_uuid"].as_str().unwrap_or_default(),
                database["database"].as_str().unwrap_or_default(),
            ]
        {
            return Err(format!("数据库 identity 核验失败：{key}").into());
        }
        let response = database::mysql(
            context,
            transport,
            &database,
            "SELECT resource_kind, scope_id, marker FROM ryframe_resource_ownership ORDER BY resource_kind;",
            "数据库 ownership 核验",
        )?;
        let kinds: &[&str] = if database["kind"].as_str() == Some("combined") {
            &["control", "tenant-data"]
        } else {
            &["tenant-data"]
        };
        let mut expected = BTreeSet::new();
        for kind in kinds {
            expected.insert(format!("{kind}\t{scope}\tryframe-owner:v1:{scope}:{kind}"));
        }
        let actual = response
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(format!("数据库 ownership 核验失败：{key}").into());
        }
    }
    Ok(())
}

fn validate_dump_state(
    context: &Context,
    models: &Models,
    dumps: &[Value],
    objects: &BTreeMap<(String, String), Value>,
) -> Result<Vec<Value>> {
    let scope = dump_scope(context, models, dumps)?;
    let (observed, schedules) = dump_rows(context, models, dumps, &scope)?;
    let object_bindings = object_bindings(objects)?;
    state::validate_relations(&observed, &scope.databases, &object_bindings)?;
    Ok(schedules)
}

struct DumpScope {
    databases: BTreeMap<String, Database>,
    tenants: BTreeSet<String>,
    forbidden: Vec<String>,
}

fn dump_scope(context: &Context, models: &Models, dumps: &[Value]) -> Result<DumpScope> {
    let databases = context
        .databases()?
        .into_iter()
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
    let artifact = context.output().join(
        control_dump["artifact"]["file"]
            .as_str()
            .ok_or("控制库转储缺少文件名")?,
    );
    let mut tenants = BTreeSet::new();
    let tenant_name = Regex::new(r"^[A-Za-z0-9_-]{1,64}$")?;
    sql::for_each_row(&artifact, &models.control, Some("sys_tenant"), |_, row| {
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
        context.source()["scope_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        context.source()["s3"]["endpoint"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    ];
    forbidden.extend(databases.values().map(|database| database.database.clone()));
    Ok(DumpScope {
        databases,
        tenants,
        forbidden,
    })
}

fn dump_rows(
    context: &Context,
    models: &Models,
    dumps: &[Value],
    scope: &DumpScope,
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
        let file = context.output().join(
            dump["artifact"]["file"]
                .as_str()
                .ok_or("数据库转储缺少文件名")?,
        );
        let mut actual = sql::empty_counts(&catalog);
        sql::for_each_row(&file, &catalog, None, |table, row| {
            validate_row(table, row, scope, &mut schedules)?;
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
    scope: &DumpScope,
    schedules: &mut Vec<Value>,
) -> Result<()> {
    if let Some(value) = row.get("tenant_id").filter(|value| !value.is_null()) {
        let tenant = value.text().ok_or("数据引用未登记的逻辑租户")?;
        if !scope.tenants.contains(tenant) {
            return Err("数据引用未登记的逻辑租户".into());
        }
    }
    state::reject_physical(row, &scope.forbidden, &scope.tenants)?;
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

fn database_from(value: Value) -> Result<Database> {
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

fn evidence(root: &Path) -> Result<Value> {
    let names = [
        "request",
        "generation-before",
        "generation-after",
        "inventory-before",
        "inventory-after",
        "inventory-before-targets",
        "inventory-after-targets",
        "schema-before",
        "schema-after",
    ];
    names
        .into_iter()
        .map(|name| {
            let file = format!("{name}.json");
            let path = root.join(&file);
            Ok((
                name.to_owned(),
                json!({"file": file, "bytes": digest::file(&path)?.bytes, "sha256": digest::file(&path)?.sha256}),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()
        .and_then(|values| Ok(serde_json::to_value(values)?))
}

fn verify_files(root: &Path, dumps: &[Value], objects: &[Value]) -> Result<()> {
    for value in dumps
        .iter()
        .map(|dump| &dump["artifact"])
        .chain(objects.iter().flat_map(|bucket| {
            bucket["entries"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|entry| [&entry["artifact"], &entry["capture"]])
        }))
    {
        let path = root.join(value["file"].as_str().ok_or("导出证据缺少文件名")?);
        let actual = digest::file(&path)?;
        if Some(actual.bytes) != value["bytes"].as_u64()
            || Some(actual.sha256.as_str()) != value["sha256"].as_str()
        {
            return Err("源导出本地文件在完整验证前变化".into());
        }
    }
    Ok(())
}
