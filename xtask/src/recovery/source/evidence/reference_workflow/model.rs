use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde_json::{Value, json};

use super::{exact, inputs, native, path, text, unlinked};
use crate::Result;

pub(super) fn manifest(args: &Value) -> Result<Value> {
    exact(args, &["plan", "inventory", "artifacts", "root"])?;
    let plan = &args["plan"];
    let inventory = &args["inventory"];
    super::inventory_shape(plan, inventory)?;
    inputs::inventory::validate(plan, inventory)?;
    let captured = DateTime::parse_from_rfc3339(text(&inventory["captured_at"])?)?;
    let completed = DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
        .ok_or("备份完成时间超出范围")?;
    if captured > completed {
        return Err("备份采集时间不能晚于备份完成时间".into());
    }
    let retention = captured
        .checked_add_signed(Duration::days(7))
        .ok_or("备份保留时间溢出")?;
    let mut value = inventory.clone();
    value["id"] = plan["id"].clone();
    // 采用产品 BackupManifest 序列化的 UTC 格式，保持备份与数据库登记值相同。
    value["completed_at"] = json!(completed.to_rfc3339_opts(SecondsFormat::AutoSi, true));
    value["retention_until"] = json!(
        retention
            .to_utc()
            .to_rfc3339_opts(SecondsFormat::AutoSi, true)
    );
    value["artifacts"] = args["artifacts"].clone();
    let root = path(&args["root"])?;
    unlinked(root)?;
    inputs::inventory::verify_artifacts(root, &value)?;
    Ok(value)
}

pub(super) fn resources(args: &Value) -> Result<Value> {
    exact(args, &["plan", "root", "manifest"])?;
    let plan = &args["plan"];
    let manifest = &args["manifest"];
    super::inventory_shape(plan, manifest)?;
    let root = path(&args["root"])?;
    unlinked(root)?;
    inputs::inventory::validate(plan, manifest)?;
    inputs::inventory::verify_artifacts(root, manifest)?;
    let mut tasks = Vec::new();
    for database in native::json::array(&manifest["databases"], "备份数据库")? {
        let key = text(&database["key"])?;
        let relative = format!("databases/{key}.sql");
        let file = super::files::bound_path(root, manifest, &relative)?;
        let tables = native::json::array(&database["tables"], "备份表")?
            .iter()
            .map(|table| table["table"].clone())
            .collect::<Vec<_>>();
        super::super::reference::validate_dump(&json!({"path":file,"tables":tables}))?;
        let target = native::json::array(&plan["target"]["databases"], "目标数据库")?
            .iter()
            .find(|database| database["key"] == key)
            .ok_or("恢复目标数据库缺失")?;
        tasks.push(json!({"kind":"database","target":target,"tables":tables,"relative":relative}));
    }
    let source = format!("{}/", text(&plan["source"]["scope_id"])?);
    let target = format!("{}/", text(&plan["target"]["scope_id"])?);
    for objects in native::json::array(&manifest["objects"], "备份对象")? {
        let index = super::super::reference::object_index(
            &json!({"root":root,"objects":objects,"artifacts":manifest["artifacts"]}),
        )?;
        for entry in native::json::array(&index["entries"], "对象条目")? {
            let key = text(&entry["key"])?;
            let suffix = key.strip_prefix(&source).ok_or("恢复对象不属于来源scope")?;
            let content_type = text(&entry["content_type"])?;
            if content_type.is_empty() || content_type.chars().any(char::is_control) {
                return Err("恢复对象ContentType无效".into());
            }
            tasks.push(json!({"kind":"object","bucket":objects["bucket"],
                "key":format!("{target}{suffix}"),"relative":entry["file"],
                "metadata":{"ContentType":content_type,"CacheControl":null,"ContentDisposition":null,
                    "ContentEncoding":null,"ContentLanguage":null,"Expires":null,"WebsiteRedirectLocation":null,"Metadata":{}}}));
        }
    }
    Ok(json!(tasks))
}

pub(super) fn bound_artifact(args: &Value) -> Result<Value> {
    exact(args, &["root", "manifest", "relative"])?;
    let root = path(&args["root"])?;
    unlinked(root)?;
    let file = super::files::bound_path(root, &args["manifest"], text(&args["relative"])?)?;
    let digest = native::digest::file(&file)?;
    let manifest = &args["manifest"];
    let artifact = native::json::array(&manifest["artifacts"], "备份产物")?
        .iter()
        .find(|entry| entry["relative_path"] == args["relative"])
        .ok_or("恢复产物未登记")?;
    if artifact["bytes"] != digest.bytes || artifact["sha256"] != digest.sha256 {
        return Err("恢复产物在绑定读取期间变化".into());
    }
    Ok(json!({"path":file,"bytes":digest.bytes,"sha256":digest.sha256}))
}

pub(super) fn summary(args: &Value) -> Result<Value> {
    exact(args, &["plan"])?;
    let plan = &args["plan"];
    let mut targets = serde_json::Map::new();
    for side in ["source", "target"] {
        let values = native::json::array(&plan[side]["databases"], "计划数据库")?.iter()
            .map(|db| {
                for field in ["key", "server_uuid", "database"] { text(&db[field])?; }
                Ok(json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]}))
            }).collect::<Result<Vec<_>>>()?;
        targets.insert(side.into(), json!(values));
    }
    Ok(
        json!({"id":plan["id"],"plan_sha256":native::digest::plan_hash(plan)?,"work_dir":plan["work_dir"],"targets":targets}),
    )
}

pub(super) fn restore_result(args: &Value) -> Result<Value> {
    exact(args, &["record", "bindings", "runtime", "images"])?;
    exact(&args["images"], &["before", "after"])?;
    let bindings = exact(
        &args["bindings"],
        &[
            "target_side",
            "target_plan",
            "backup_receipt",
            "record",
            "runtime_registration",
        ],
    )?;
    let mut result = bindings.clone();
    result.extend(native::json::object(&json!({"restore_id":args["record"]["plan"]["id"],
        "status":"external_copy_completed","runtime":args["runtime"],"resource_images":args["images"],
        "next":"执行 restore-verify-data；外部复制完成不代表恢复成功"}), "恢复结果")?.clone());
    Ok(Value::Object(result))
}

pub(super) fn preflight(args: &Value) -> Result<Value> {
    exact(args, &["plan"])?;
    let plan = &args["plan"];
    Ok(
        json!({"format_version":1,"kind":"restore-reference-dataset-preflight",
        "plan_sha256":native::digest::plan_hash(plan)?,"side":"source",
        "scope_id":plan["source"]["scope_id"],"actions":{"business":"empty_source_verified"}}),
    )
}

pub(super) fn backup_inventory(
    plan: &Value,
    raw: &Value,
    source: impl FnOnce() -> Result<Value>,
) -> Result<Value> {
    let mut inventory = native::json::object(raw, "共享导出原始库存")?.clone();
    if inventory.contains_key("id") {
        return Err("backup必须直接消费共享导出的原始库存，不能手工补写id".into());
    }
    let source = source()?;
    if source["clean"].as_bool() != Some(true) || source["head"] != inventory["source_sha"] {
        return Err("正式备份协调器必须是共享导出库存source_sha对应的干净工作树".into());
    }
    inventory.insert("id".into(), plan["id"].clone());
    Ok(Value::Object(inventory))
}
