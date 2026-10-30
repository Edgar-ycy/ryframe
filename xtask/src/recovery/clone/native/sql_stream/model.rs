use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use super::super::{catalog::Catalog, digest, json as json_io};
use crate::Result;
use serde_json::{Value, json};

pub(super) const KIND: &str = "ryframe-sql-row-stream-v1";
pub(super) const FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "backend",
    "root",
    "source",
    "catalog_sha256",
    "only_table",
    "tables",
    "source_rows",
    "rows",
    "artifact",
];

pub(super) fn exact(value: &Value, fields: &[&str]) -> Result<()> {
    json_io::exact(value, fields, "SQL 流合同")?;
    Ok(())
}

pub(super) fn text(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "SQL 流字符串字段无效".into())
}

pub(super) fn sha(value: &Value) -> Result<&str> {
    let value = text(value)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("SQL 流 SHA256 格式无效".into());
    }
    Ok(value)
}

pub(super) fn plain(path: &Path, directory: bool) -> Result<()> {
    if !path.is_absolute() || digest::linked_path(path)?.into_iter().any(|linked| linked) {
        return Err("SQL 流路径必须绝对且不能经过链接".into());
    }
    let metadata = fs::symlink_metadata(path)?;
    if if directory {
        !metadata.is_dir()
    } else {
        !metadata.is_file()
    } {
        return Err("SQL 流路径类型无效".into());
    }
    Ok(())
}

pub(super) fn path(value: &Value, directory: bool) -> Result<PathBuf> {
    let path = PathBuf::from(text(value)?);
    plain(&path, directory)?;
    Ok(path)
}

pub(super) fn catalog(value: &Value) -> Result<Catalog> {
    let catalog: Catalog =
        serde_json::from_value(value.clone()).map_err(|_| "SQL 流目录结构无效")?;
    for (table, columns) in &catalog {
        if !identifier(table, true)
            || columns.is_empty()
            || columns
                .iter()
                .any(|(name, kind)| !identifier(name, false) || kind.trim().is_empty())
        {
            return Err("SQL 流表名、列名或类型无效".into());
        }
    }
    Ok(catalog)
}

fn identifier(value: &str, table: bool) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || (table && b == b'-')
        })
}

pub(super) fn owned_parent(backend: &Path) -> Result<PathBuf> {
    plain(backend, true)?;
    let local = backend.join(".local-tests");
    plain(&local, true)?;
    let parent = local.join("sql-row-streams");
    match fs::create_dir(&parent) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    plain(&parent, true)?;
    Ok(fs::canonicalize(parent)?)
}

pub(super) fn descriptor(value: &Value) -> Result<PathBuf> {
    let mut fields = FIELDS.to_vec();
    fields.push("manifest");
    exact(value, &fields)?;
    if value["format_version"] != 1 || value["kind"] != KIND {
        return Err("SQL 流描述类型无效".into());
    }
    let backend = path(&value["backend"], true)?;
    if fs::canonicalize(&backend)? != backend {
        return Err("SQL 流后端未规范化".into());
    }
    let parent = backend.join(".local-tests/sql-row-streams");
    plain(&parent, true)?;
    let root = path(&value["root"], true)?;
    let prefix = root
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("sql-rows-") && name.len() > 9);
    if root.parent() != Some(parent.as_path()) || fs::canonicalize(&root)? != root || !prefix {
        return Err("SQL 流目录不属于固定受管根".into());
    }
    sha(&value["catalog_sha256"])?;
    file_descriptor(&value["source"], "path", None)?;
    let source = path(&value["source"]["path"], false)?;
    if source.starts_with(&root) {
        return Err("SQL 流源文件不能属于结果目录".into());
    }
    file_descriptor(&value["artifact"], "file", Some("rows.jsonl"))?;
    file_descriptor(&value["manifest"], "file", Some("manifest.json"))?;
    if value["source"]["bytes"].as_u64() == Some(0)
        || value["manifest"]["bytes"].as_u64() == Some(0)
    {
        return Err("SQL 流源或清单不能为空".into());
    }
    counts(value)?;
    members(&root)?;
    Ok(root)
}

fn file_descriptor(value: &Value, key: &str, expected: Option<&str>) -> Result<()> {
    exact(value, &[key, "bytes", "sha256", "device", "inode"])?;
    let name = text(&value[key])?;
    if expected.is_some_and(|expected| name != expected)
        || ["bytes", "device", "inode"]
            .iter()
            .any(|key| value[*key].as_u64().is_none())
    {
        return Err("SQL 流文件描述无效".into());
    }
    sha(&value["sha256"])?;
    Ok(())
}

fn counts(value: &Value) -> Result<()> {
    let tables = value["tables"].as_object().ok_or("SQL 流表计数无效")?;
    let mut total = 0_u64;
    for (table, count) in tables {
        if !identifier(table, true) {
            return Err("SQL 流表名无效".into());
        }
        total = total
            .checked_add(count.as_u64().ok_or("SQL 流表计数无效")?)
            .ok_or("SQL 流计数溢出")?;
    }
    let selected = if value["only_table"].is_null() {
        total
    } else {
        tables
            .get(text(&value["only_table"])?)
            .and_then(Value::as_u64)
            .ok_or("SQL 流过滤表未登记")?
    };
    if value["source_rows"] != json!(total) || value["rows"] != json!(selected) {
        return Err("SQL 流总行数不符".into());
    }
    Ok(())
}

pub(super) fn members(root: &Path) -> Result<()> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "SQL 流目录成员无效")?;
        if !["rows.jsonl", "manifest.json"].contains(&name.as_str()) {
            return Err("SQL 流目录含未登记成员".into());
        }
        digest::safe_file(root, &name)?;
        names.insert(name);
    }
    if names != BTreeSet::from(["rows.jsonl".to_owned(), "manifest.json".to_owned()]) {
        return Err("SQL 流目录成员不完整".into());
    }
    Ok(())
}
