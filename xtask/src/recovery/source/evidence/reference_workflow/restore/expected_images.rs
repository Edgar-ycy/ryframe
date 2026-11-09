//! 从已验证的计划、备份和目标初始化推导完整恢复资源像。
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::{
        clone::native::{
            digest::plan_hash, s3::protocol::validate_key,
            target_verify::database::model::fingerprints,
        },
        inputs::{
            document::{array, digest, integer, object, text},
            domain::{BUCKETS, EXCLUDED_TABLES, scope_identifier},
        },
    },
};

pub(super) fn expected_images(
    plan: &Value,
    manifest: &Value,
    initialized: &Value,
) -> Result<(Value, Value)> {
    let source = text(&plan["source"]["scope_id"], "恢复源 scope")?;
    let target = text(&plan["target"]["scope_id"], "恢复目标 scope")?;
    scope_identifier(source)?;
    scope_identifier(target)?;
    if source == target || manifest["scope_id"] != source {
        return Err("恢复源、目标和备份的 scope 不一致或重叠".into());
    }
    let databases = &initialized["inventory"]["observations"];
    let objects = &initialized["objects"];
    object(databases, "初始化数据库观察")?;
    object(objects, "初始化对象观察")?;
    let redis = initialized.get("redis").ok_or("初始化缺少 Redis 前像")?;
    let before = json!({"databases":databases,"objects":objects,"redis":redis});
    let mut after = before.clone();
    project_databases(plan, manifest, target, &mut after["databases"])?;
    project_objects(manifest, source, target, &mut after["objects"])?;
    Ok((before, after))
}

fn databases<'a>(value: &'a Value, label: &str) -> Result<BTreeMap<&'a str, &'a Value>> {
    let mut databases = BTreeMap::new();
    for row in array(value, label)? {
        let key = text(&row["key"], label)?;
        if databases.insert(key, row).is_some() {
            return Err("恢复数据库计划键重复".into());
        }
    }
    Ok(databases)
}

fn project_databases(
    plan: &Value,
    manifest: &Value,
    scope: &str,
    images: &mut Value,
) -> Result<()> {
    let sources = databases(&plan["source"]["databases"], "恢复源数据库")?;
    let targets = databases(&plan["target"]["databases"], "恢复目标数据库")?;
    let backup = databases(&manifest["databases"], "备份数据库")?;
    if !sources.keys().eq(targets.keys()) || !targets.keys().eq(backup.keys()) {
        return Err("正式备份必须完整覆盖计划中的数据库键".into());
    }
    let names = targets
        .values()
        .map(|database| text(&database["database"], "恢复目标物理库名"))
        .collect::<Result<BTreeSet<_>>>()?;
    let observed = object(images, "初始化数据库观察")?;
    if names.len() != targets.len()
        || !names
            .iter()
            .copied()
            .eq(observed.keys().map(String::as_str).collect::<BTreeSet<_>>())
    {
        return Err("初始化数据库观察与目标物理库名不同或重复".into());
    }
    let mut schema = fingerprints(manifest)?;
    let combined = plan_hash(&schema)?;
    schema["control_schema_fingerprint"] = Value::Null;
    let tenant = plan_hash(&schema)?;
    for (key, target) in targets {
        let database = backup[key];
        let source = sources[key];
        if ["database", "server_uuid", "kind"]
            .iter()
            .any(|field| database[*field] != source[*field])
            || source["kind"] != target["kind"]
        {
            return Err("备份数据库物理身份或类型与计划不同".into());
        }
        let schema = match target["kind"].as_str() {
            Some("combined") => &combined,
            Some("tenant") => &tenant,
            _ => return Err("恢复目标数据库类型无效".into()),
        };
        let name = text(&target["database"], "恢复目标物理库名")?;
        let image = &mut images[name];
        let resource = &image["resource"];
        if resource["kind"] != "database"
            || resource["scope_id"] != scope
            || resource["database"] != target["database"]
            || resource["server_uuid"] != target["server_uuid"]
            || image["schema_sha256"].as_str() != Some(schema.as_str())
        {
            return Err("恢复目标数据库物理身份或 schema 与初始化观察不同".into());
        }
        project_tables(image, database)?;
    }
    Ok(())
}

fn project_tables(image: &mut Value, database: &Value) -> Result<()> {
    let all = array(&image["all_tables"], "初始化完整表集合")?;
    let names = all
        .iter()
        .map(|value| text(value, "初始化表名"))
        .collect::<Result<BTreeSet<_>>>()?;
    let tables = object(&image["tables"], "初始化业务表")?;
    let preserved = object(&image["preserved"], "初始化保留表")?;
    let observed = tables
        .keys()
        .chain(preserved.keys())
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if names.len() != all.len()
        || observed != names
        || observed.len() != tables.len() + preserved.len()
    {
        return Err("初始化业务表和保留表未精确分割完整表集合".into());
    }
    let mut restored = BTreeMap::new();
    for table in array(&database["tables"], "备份表")? {
        let name = text(&table["table"], "备份表名")?;
        integer(&table["rows"], "备份表行数")?;
        digest(&table["sha256"], "备份表摘要")?;
        let value = json!({"rows":table["rows"],"sha256":table["sha256"]});
        if restored.insert(name, value).is_some() {
            return Err("备份数据库包含重复表".into());
        }
    }
    if !restored.keys().copied().eq(names
        .into_iter()
        .filter(|name| !EXCLUDED_TABLES.contains(name)))
    {
        return Err("正式备份必须覆盖目标全部非保留表".into());
    }
    for field in ["tables", "preserved"] {
        let rows = image[field].as_object_mut().ok_or("初始化表观察无效")?;
        for (name, current) in rows {
            if let Some(value) = restored.get(name.as_str()) {
                *current = value.clone();
            }
        }
    }
    Ok(())
}

fn project_objects(manifest: &Value, source: &str, target: &str, images: &mut Value) -> Result<()> {
    let images = images.as_object_mut().ok_or("初始化对象观察无效")?;
    if images.len() != BUCKETS.len() || BUCKETS.iter().any(|bucket| !images.contains_key(*bucket)) {
        return Err("初始化对象观察必须覆盖五个业务桶".into());
    }
    let source_prefix = format!("{source}/");
    let owner = format!("{target}/.ryframe-owner");
    let mut buckets = BTreeSet::new();
    for bucket in array(&manifest["objects"], "备份对象桶")? {
        let name = text(&bucket["bucket"], "备份对象桶名")?;
        if !BUCKETS.contains(&name) || !buckets.insert(name) || bucket["prefix"] != source_prefix {
            return Err("备份对象桶未登记、重复或不属于源 scope".into());
        }
        let image = images.get_mut(name).ok_or("初始化缺少对象桶观察")?;
        if image["keys"] != json!([owner]) {
            return Err("初始化对象范围必须只含原目标 owner key".into());
        }
        object(&image["owner"], "初始化对象 owner")?;
        let mut keys = BTreeSet::from([owner.clone()]);
        for entry in array(&bucket["entries"], "备份对象条目")? {
            let key = text(&entry["key"], "备份对象 key")?;
            validate_key(source, key, false)?;
            let suffix = key
                .strip_prefix(&source_prefix)
                .ok_or("备份对象不属于源 scope")?;
            let key = format!("{target}/{suffix}");
            validate_key(target, &key, false)?;
            if !keys.insert(key) {
                return Err("备份同桶对象 key 重复".into());
            }
        }
        image["keys"] = json!(keys);
    }
    if buckets.len() != BUCKETS.len() {
        return Err("正式备份必须覆盖五个业务桶".into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/source/evidence/reference_workflow/restore/expected_images_tests.rs"]
mod tests;
