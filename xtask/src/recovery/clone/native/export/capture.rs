use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Value, json};
use time::OffsetDateTime;

use super::super::{digest, json as json_io, resources::storage::registered};
use super::{
    database::{self, Models},
    input::Context,
    inventory, object, proof, recovery,
    transport::{SystemTransport, Transport},
    verify,
};
use crate::{Result, cli::CloneSide};

pub(super) mod dump;
use dump::{PhysicalRules, validate_dump_state};

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
    match capture(context) {
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

fn capture(context: &Context) -> Result<Value> {
    let storage_before =
        registered::current(&context.backend, &context.run_dir, CloneSide::Source)?;
    let check_storage = || {
        if registered::current(&context.backend, &context.run_dir, CloneSide::Source)?
            != storage_before
        {
            return Err("源 storage provenance 在导出前后发生变化".into());
        }
        Ok(())
    };
    let exported = capture_data(
        context,
        &SystemTransport,
        &|| proof::generation(context),
        &check_storage,
    )?;
    let verified = verify::binding(context, &exported)?;
    recovery::record_verified(context, &verified)?;
    Ok(json!({
        "status": "export_verified",
        "export": exported,
        "logical_inventory_sha256": verified["export"]["logical_inventory_sha256"],
        "remote_writes": 0,
        "restore_qualified": false,
    }))
}

pub(crate) fn capture_data(
    context: &Context,
    transport: &impl Transport,
    generation: &dyn Fn() -> Result<Value>,
    check_storage: &dyn Fn() -> Result<()>,
) -> Result<Value> {
    let request_digest = digest::file(&context.request_path)?;
    let frame = capture_before(context, transport, generation)?;
    capture_after(context, transport, &frame, generation)?;
    check_storage()?;
    let output = context.output();
    verify_files(&output, &frame.dumps, &frame.objects)?;
    if digest::file(&context.request_path)? != request_digest {
        return Err("源导出请求文件在执行期间变化".into());
    }
    let export = build_export(context, &frame, request_digest)?;
    let path = output.join("export.json");
    json_io::write_new(&path, &export)?;
    super::super::resources::common::binding(&path)
}

fn capture_before(
    context: &Context,
    transport: &impl Transport,
    generation: &dyn Fn() -> Result<Value>,
) -> Result<CaptureFrame> {
    let output = context.output();
    json_io::write_new(
        &output.join("request.json"),
        &super::input::public_request(&context.backend, &context.request)?,
    )?;
    let before = generation()?;
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
    ensure_generation(generation, &before)?;
    let quiesced = crate::utc_timestamp::rfc3339_micros(OffsetDateTime::now_utc());
    let inventory_before =
        inventory::capture(context, transport, &models, "before", &quiesced, false)?;
    let before_index = inventory::object_index(&inventory_before)?;
    let dumps = capture_dumps(context, transport, &models)?;
    ensure_generation(generation, &before)?;
    let endpoint = json_io::text(&context.object_identity["endpoint"], "源对象端点")?;
    let schedules_before = validate_dump_state(
        context.source(),
        endpoint,
        &output,
        &models,
        &dumps,
        &before_index,
        PhysicalRules::Capture,
    )?
    .schedules;
    let (objects, object_index) = object::capture(
        context,
        transport,
        &inventory_before,
        context.request["max_object_bytes"]
            .as_u64()
            .ok_or("源导出缺少对象大小上限")?,
    )?;
    let schedules = validate_dump_state(
        context.source(),
        endpoint,
        &output,
        &models,
        &dumps,
        &object_index,
        PhysicalRules::Capture,
    )?
    .schedules;
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

pub(crate) fn capture_dumps<T: Transport>(
    context: &Context,
    transport: &T,
    models: &Models,
) -> Result<Vec<Value>> {
    let directory = context.output().join("databases");
    fs::create_dir(&directory).map_err(|error| {
        format!(
            "无法独占创建本次数据库转储目录 {}：{error}；保留现场，不覆盖或重放",
            directory.display()
        )
    })?;
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
    transport: &impl Transport,
    frame: &CaptureFrame,
    generation: &dyn Fn() -> Result<Value>,
) -> Result<()> {
    ensure_generation(generation, &frame.before)?;
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
    let after = generation()?;
    json_io::write_new(&context.output().join("generation-after.json"), &after)?;
    if frame.before != after {
        return Err("源导出期间源码、构建、配置、producer 或工具代次变化".into());
    }
    Ok(())
}

fn ensure_generation(generation: &dyn Fn() -> Result<Value>, expected: &Value) -> Result<()> {
    if generation()? != *expected {
        return Err("源导出期间源码、构建、配置、producer 或工具代次变化".into());
    }
    Ok(())
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
        "source": super::input::public_request(&context.backend, &context.request)?["source"],
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
        "completed_at": crate::utc_timestamp::rfc3339_micros(OffsetDateTime::now_utc()),
        "requires_offline_plan": true,
        "operator_declared_producers_only": true,
        "remote_writes": 0,
        "clone_verified": false,
        "restore_qualified": false,
        "dump_semantic_digest_verified": false,
        "dump_validation": "固定外部工具与明确源连接、完整列、逐表行数、文件 SHA；实际目标导入后仍须匹配源 inventory 逻辑摘要",
    }))
}

fn verify_databases(context: &Context, transport: &impl Transport, _models: &Models) -> Result<()> {
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
