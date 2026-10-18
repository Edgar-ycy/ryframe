use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    document::{array, boolean, digest, exact, integer, linked, object, text},
    domain::{BUCKETS, EXCLUDED_TABLES, file_sha256, file_size, identifier, timestamp},
};
use crate::Result;

pub(crate) fn validate(plan: &Value, inventory: &Value) -> Result<()> {
    let inventory = object(inventory, "备份清单")?;
    if inventory.contains_key("observed_at") || !inventory.contains_key("quiesced_at") {
        return Err("正式备份必须使用停机库存，不能采用运行中观察库存".into());
    }
    timestamp(inventory.get("quiesced_at").unwrap(), "备份停机时间")?;
    if text(inventory.get("id").unwrap(), "备份清单 ID")?
        != text(plan.get("id").unwrap(), "参考计划 ID")?
        || text(inventory.get("scope_id").unwrap(), "备份清单 scope")?
            != text(
                plan.get("source")
                    .and_then(|value| value.get("scope_id"))
                    .ok_or("参考计划缺少源 scope")?,
                "源 scope",
            )?
    {
        return Err("备份清单 ID 或 scope 与资源计划不一致".into());
    }
    let expected = array(
        plan.get("source")
            .and_then(|value| value.get("databases"))
            .ok_or("参考计划缺少源数据库")?,
        "源数据库",
    )?
    .iter()
    .map(|database| {
        (
            text(database.get("key").unwrap(), "源数据库键").unwrap(),
            database,
        )
    })
    .collect::<HashMap<_, _>>();
    let databases = array(
        inventory.get("databases").ok_or("备份清单缺少数据库")?,
        "备份数据库",
    )?;
    if databases.len() != expected.len()
        || databases
            .iter()
            .map(|database| text(database.get("key").unwrap(), "备份数据库键"))
            .collect::<Result<HashSet<_>>>()?
            != expected.keys().copied().collect()
    {
        return Err("备份清单必须完整覆盖精确数据库目标".into());
    }
    for database in databases {
        exact(
            database,
            &["key", "database", "server_uuid", "kind", "shared", "tables"],
            "备份数据库",
        )?;
        let key = text(database.get("key").unwrap(), "备份数据库键")?;
        let source = expected.get(key).ok_or("备份数据库不在参考计划")?;
        for field in ["database", "server_uuid", "kind"] {
            if database.get(field) != source.get(field) {
                return Err("备份数据库与计划中的物理身份不匹配".into());
            }
        }
        if boolean(database.get("shared").unwrap(), "备份共享模式")?
            != (source.get("mode").and_then(Value::as_str) == Some("shared"))
        {
            return Err("备份数据库共享模式与计划不匹配".into());
        }
        let mut tables = HashSet::new();
        for table in array(database.get("tables").unwrap(), "备份表")? {
            let name = text(
                object(table, "备份表")?
                    .get("table")
                    .ok_or("备份表缺少 table")?,
                "备份表名",
            )?;
            identifier(name)?;
            if !tables.insert(name) || EXCLUDED_TABLES.contains(&name) {
                return Err("备份表缺失、重复或包含必须保留的目标元数据".into());
            }
        }
        if tables.is_empty() {
            return Err("备份表缺失、重复或包含必须保留的目标元数据".into());
        }
    }
    let objects = array(
        inventory.get("objects").ok_or("备份清单缺少对象")?,
        "备份对象",
    )?;
    let names = objects
        .iter()
        .map(|value| text(value.get("bucket").unwrap(), "对象桶"))
        .collect::<Result<HashSet<_>>>()?;
    if objects.len() != BUCKETS.len() || names != BUCKETS.into_iter().collect() {
        return Err("备份清单必须恰好包含五个业务对象桶".into());
    }
    let prefix = format!(
        "{}/",
        text(inventory.get("scope_id").unwrap(), "备份 scope")?
    );
    for objects in objects {
        exact(objects, &["bucket", "prefix", "entries"], "备份对象桶")?;
        if text(objects.get("prefix").unwrap(), "对象前缀")? != prefix {
            return Err("对象清单越过明确 scope、重复或包含 ownership 标记".into());
        }
        let mut keys = HashSet::new();
        for entry in array(objects.get("entries").unwrap(), "对象条目")? {
            let entry = object(entry, "对象条目")?;
            let key = text(entry.get("key").ok_or("对象条目缺少 key")?, "对象键")?;
            let sha = digest(entry.get("sha256").ok_or("对象条目缺少摘要")?, "对象摘要")?;
            let bytes = integer(entry.get("bytes").ok_or("对象条目缺少大小")?, "对象大小")?;
            if !keys.insert(key)
                || !key.starts_with(&prefix)
                || key == format!("{prefix}.ryframe-owner")
                || key.chars().any(|character| (character as u32) < 32)
                || bytes == 0
            {
                return Err("对象清单越过明确 scope、重复或包含 ownership 标记".into());
            }
            digest(&Value::String(sha), "对象摘要")?;
        }
    }
    Ok(())
}

pub(crate) fn verify_artifacts(root: &Path, manifest: &Value) -> Result<()> {
    let manifest = object(manifest, "备份清单")?;
    let databases = array(
        manifest.get("databases").ok_or("备份清单缺少数据库")?,
        "备份数据库",
    )?;
    let mut required = databases
        .iter()
        .map(|database| {
            Ok(format!(
                "db:{}",
                text(database.get("key").unwrap(), "数据库键")?
            ))
        })
        .collect::<Result<HashSet<_>>>()?;
    required.extend(
        BUCKETS
            .into_iter()
            .map(|bucket| format!("objects:{bucket}")),
    );
    let mut resources = HashSet::new();
    let mut paths = HashSet::new();
    for artifact in array(
        manifest.get("artifacts").ok_or("备份清单缺少产物")?,
        "备份产物",
    )? {
        let artifact = exact(
            artifact,
            &["relative_path", "resource", "bytes", "sha256"],
            "备份产物",
        )?;
        let relative = text(artifact.get("relative_path").unwrap(), "产物路径")?;
        let resource = text(artifact.get("resource").unwrap(), "产物资源")?;
        if !paths.insert(relative.to_owned()) || !required.contains(resource) {
            return Err("备份产物重复或引用未知目标".into());
        }
        let path = safe_file(root, relative)?;
        let bytes = integer(artifact.get("bytes").unwrap(), "产物大小")?;
        let sha = digest(artifact.get("sha256").unwrap(), "产物摘要")?;
        if file_sha256(&path)? != sha || file_size(&path)? != bytes || bytes == 0 {
            return Err("备份文件大小或摘要被篡改".into());
        }
        resources.insert(resource.to_owned());
    }
    if resources != required {
        return Err("备份产物未覆盖全部数据库和对象范围".into());
    }
    Ok(())
}

fn safe_file(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.chars().any(|character| (character as u32) < 32)
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("备份文件路径越界或格式无效".into());
    }
    let candidate = root.join(relative);
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        current.push(component.as_os_str());
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| "备份文件缺失或无法核验".to_owned())?;
        if linked(&metadata) {
            return Err("备份文件通过链接越界".into());
        }
    }
    if !fs::canonicalize(&candidate)?.starts_with(fs::canonicalize(root)?) {
        return Err("备份文件通过链接越界".into());
    }
    Ok(candidate)
}
