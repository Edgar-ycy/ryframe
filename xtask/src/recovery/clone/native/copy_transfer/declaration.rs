//! 从已核导出和两侧真实观察组装离线计划输入。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, is_separator},
};

use serde_json::{Value, json};
use uuid::Uuid;

use super::super::{
    input::{BUCKETS, digest_shape, name},
    json as document,
    s3::protocol::validate_key,
    schedule,
    target_verify::{database::model::expected_owners, input::request_physical::endpoint},
};
use crate::{Result, recovery::inputs::domain};

pub(crate) struct Declaration<'a> {
    pub(crate) export: &'a Value,
    pub(crate) source: &'a BTreeMap<String, Value>,
    pub(crate) target: &'a Value,
    pub(crate) target_endpoint: &'a str,
    pub(crate) initialized: &'a BTreeMap<String, Value>,
    pub(crate) evidence: &'a Value,
    pub(crate) copy_id: &'a str,
    pub(crate) stage: &'a str,
    pub(crate) execution_backend: &'a Path,
}

/// 调用方继续使用正式离线入口验证原文件、SQL、关系和目标处置。
pub(crate) fn compose(spec: Declaration<'_>) -> Result<Value> {
    name(spec.copy_id)?;
    let root = document::text(&spec.export["artifact_root"], "原导出证据根")?;
    absolute(Path::new(root), "原导出证据根")?;
    let execution = absolute(spec.execution_backend, "原导出执行后端")?;
    let source_scope = scope(&spec.export["source"])?;
    let target_scope = scope(spec.target)?;
    if source_scope == target_scope || spec.export["scope_id"] != source_scope {
        return Err("复制声明须绑定原来源 scope 和不同的目标 scope".into());
    }

    let mut physical = BTreeSet::new();
    let source = declared_side(
        &spec.export["source"],
        &spec.export["source"]["s3"]["endpoint"],
        spec.source,
        &mut physical,
    )?;
    let mut target = declared_side(
        spec.target,
        &json!(spec.target_endpoint),
        spec.initialized,
        &mut physical,
    )?;
    for (key, observed) in spec.source {
        if observed["schema_sha256"] != spec.initialized[key]["schema_sha256"] {
            return Err("复制声明两侧对应数据库的 schema 不同".into());
        }
    }
    target["state"] = json!("new_initialized");
    target["ever_started"] = json!(false);
    check_evidence(spec.evidence, spec.export)?;
    let dumps = &spec.export["databases"];
    check_dumps(dumps, spec.source)?;
    let objects = objects(&spec.export["objects"], source_scope, target_scope)?;
    let value = json!({
        "format_version":1,"copy_id":spec.copy_id,"copy_stage":spec.stage,"app_env":"test",
        "execution_backend":execution,"target_schedule_actions":schedule_rows(spec.export)?,
        "artifact_root":root,"source_snapshot":source_snapshot(spec.export)?,
        "source":source,"target":target,"evidence":spec.evidence,"databases":dumps,"objects":objects,
    });
    schedule::actions(&value)?;
    Ok(value)
}

fn scope(value: &Value) -> Result<&str> {
    name(document::text(&value["scope_id"], "复制侧 scope")?)
}

fn declared_side(
    config: &Value,
    object_endpoint: &Value,
    observations: &BTreeMap<String, Value>,
    physical: &mut BTreeSet<(String, String)>,
) -> Result<Value> {
    let scope = scope(config)?;
    endpoint(object_endpoint)?;
    let databases = document::array(&config["databases"], "复制侧登记数据库")?;
    if databases.len() != domain::TARGET_DATABASES.len() || observations.len() != databases.len() {
        return Err("复制声明每侧必须完整覆盖四个登记数据库".into());
    }
    let mut seen = BTreeSet::new();
    let mut result = Vec::with_capacity(databases.len());
    for database in databases {
        let key = document::text(&database["key"], "复制数据库逻辑 key")?;
        let (kind, mode) = domain::mode_for_key(key)?;
        let observed = observations.get(key).ok_or("实际库存缺少登记逻辑库")?;
        let server = document::text(&database["server_uuid"], "复制数据库 UUID")?;
        let db = document::text(&database["database"], "复制物理数据库名")?;
        if database["kind"] != kind
            || database["mode"] != mode
            || !seen.insert(key)
            || !physical.insert((server.to_owned(), db.to_owned()))
            || !Uuid::parse_str(server).is_ok_and(|uuid| uuid.hyphenated().to_string() == server)
            || db.is_empty()
            || db.len() > 64
            || !db
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err("复制数据库逻辑或物理身份无效、重复或与另一侧重叠".into());
        }
        let expected =
            json!({"kind":"database","scope_id":scope,"server_uuid":server,"database":db});
        document::exact(
            observed,
            &[
                "resource",
                "schema_sha256",
                "tables",
                "preserved",
                "all_tables",
                "ownership",
            ],
            "复制声明完整库存",
        )?;
        let schema = digest_shape(&observed["schema_sha256"])?;
        if observed["resource"] != expected
            || observed["ownership"].as_array() != Some(&expected_owners(scope, kind == "combined"))
        {
            return Err("实际库存的物理身份或各侧 ownership 与登记声明不符".into());
        }
        result.push(
            json!({"key":key,"kind":kind,"mode":mode,"server_uuid":server,"database":db,
            "schema_sha256":schema,"ownership":observed["ownership"]}),
        );
    }
    Ok(json!({"scope_id":scope,"object_endpoint":object_endpoint,"databases":result}))
}

fn absolute<'a>(path: &'a Path, label: &str) -> Result<&'a str> {
    let value = path
        .to_str()
        .ok_or_else(|| format!("{label}必须是 UTF-8"))?;
    if !path.is_absolute()
        || value.chars().any(char::is_control)
        || value
            .split(is_separator)
            .any(|part| matches!(part, "." | ".."))
    {
        return Err(format!("{label}必须是无跳转的绝对路径").into());
    }
    Ok(value)
}

fn source_snapshot(export: &Value) -> Result<Value> {
    let fingerprint = document::text(&export["worktree_fingerprint"], "源 DevEx 工作树指纹")?
        .strip_prefix("sha256:")
        .ok_or("源导出缺少实际 DevEx 工作区指纹")?;
    let snapshot = &export["source_snapshot"];
    digest_shape(&json!(fingerprint))?;
    Ok(json!({
        "head":domain::hash_40(&snapshot["head"], "源源码快照")?,
        "clean":document::boolean(&snapshot["clean"], "源源码快照 clean")?,
        "worktree_sha256":fingerprint,
    }))
}

fn check_evidence(evidence: &Value, export: &Value) -> Result<()> {
    let bindings = document::exact(
        evidence,
        &["source_stopped", "target_initialized", "target_stopped"],
        "复制声明证据",
    )?;
    if evidence["source_stopped"] != export["evidence"]["generation-after"] {
        return Err("复制声明的源停止证明与原导出不同".into());
    }
    for descriptor in bindings.values() {
        artifact(descriptor)?;
    }
    Ok(())
}

fn check_dumps(value: &Value, source: &BTreeMap<String, Value>) -> Result<()> {
    let dumps = document::array(value, "原导出 SQL 声明")?;
    let mut keys = BTreeSet::new();
    for dump in dumps {
        document::exact(dump, &["key", "tables", "artifact"], "原导出 SQL 条目")?;
        let key = document::text(&dump["key"], "原导出 SQL 逻辑 key")?;
        if !source.contains_key(key) || !keys.insert(key) {
            return Err("原导出 SQL 必须覆盖不同的登记逻辑库".into());
        }
        if document::object(&dump["tables"], "原导出 SQL 行数")?
            .values()
            .any(|rows| rows.as_u64().is_none())
        {
            return Err("原导出 SQL 行数必须是非负整数".into());
        }
        artifact(&dump["artifact"])?;
    }
    if keys.len() != source.len() {
        return Err("原导出 SQL 缺少登记逻辑库".into());
    }
    Ok(())
}

fn objects(value: &Value, source: &str, target: &str) -> Result<Vec<Value>> {
    let buckets = document::array(value, "原导出对象桶")?;
    let mut seen = BTreeSet::new();
    let mut result = Vec::with_capacity(buckets.len());
    let prefix = format!("{source}/");
    for bucket in buckets {
        let name = document::text(&bucket["bucket"], "原导出对象桶名")?;
        if !BUCKETS.contains(&name) || !seen.insert(name) {
            return Err("原导出对象声明的桶越界或重复".into());
        }
        let mut keys = BTreeSet::new();
        let mut entries = Vec::new();
        for item in document::array(&bucket["entries"], "原导出对象条目")? {
            let key = document::text(&item["key"], "原导出对象 key")?;
            validate_key(source, key, false)?;
            let logical = key
                .strip_prefix(&prefix)
                .ok_or("原导出对象不属于来源 scope")?;
            validate_key(target, &format!("{target}/{logical}"), false)?;
            if !keys.insert(key) {
                return Err("原导出同桶对象 key 重复".into());
            }
            artifact(&item["artifact"])?;
            document::object(&item["metadata"], "原导出对象 metadata")?;
            entries
                .push(json!({"key":key,"artifact":item["artifact"],"metadata":item["metadata"]}));
        }
        result.push(json!({"bucket":name,"entries":entries}));
    }
    if seen.len() != BUCKETS.len() {
        return Err("原导出对象声明必须完整列出五桶，包括空桶".into());
    }
    Ok(result)
}

fn artifact(value: &Value) -> Result<()> {
    document::exact(value, &["file", "bytes", "sha256"], "复制声明原文件绑定")?;
    let file = document::text(&value["file"], "复制声明相对文件路径")?;
    if file.contains(['\\', ':'])
        || file.chars().any(char::is_control)
        || file
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        || value["bytes"].as_u64().is_none_or(|bytes| bytes == 0)
    {
        return Err("复制声明原文件路径或字节数无效".into());
    }
    digest_shape(&value["sha256"])?;
    Ok(())
}

fn schedule_rows(export: &Value) -> Result<Vec<Value>> {
    document::array(&export["enabled_system_schedule_rows"], "原导出启用调度")?
        .iter()
        .map(|item| {
            let mut row = document::exact(
                item,
                &[
                    "tenant_id",
                    "schedule_id",
                    "handler_key",
                    "expected_version",
                    "source_row_sha256",
                ],
                "原导出启用调度行",
            )?
            .clone();
            row.insert("action".into(), json!("disable_schedule_via_api"));
            Ok(Value::Object(row))
        })
        .collect()
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/declaration.rs"]
mod tests;
