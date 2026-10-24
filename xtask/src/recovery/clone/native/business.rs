use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;
use serde_json::Value;
use toml::Value as Toml;

use super::{
    digest,
    json::{self, array, boolean, exact, object, text},
};
use crate::Result;

type Columns = std::collections::BTreeMap<String, String>;

pub(crate) struct Loaded {
    pub(crate) columns: std::collections::BTreeMap<String, Columns>,
    pub(crate) tables: Vec<Value>,
    pub(crate) sources: Vec<PathBuf>,
}

pub(crate) fn load(root: &Path) -> Result<Loaded> {
    let manifest = root.join("Cargo.toml");
    if !manifest.is_file() {
        return Ok(Loaded {
            columns: Default::default(),
            tables: Vec::new(),
            sources: Vec::new(),
        });
    }
    let manifest = read_regular(&manifest)?;
    let document = toml::from_str::<toml::Table>(
        std::str::from_utf8(&manifest).map_err(|_| "Workspace 清单必须是 UTF-8")?,
    )
    .map_err(|error| format!("Workspace 清单无效：{error}"))?;
    let members = document
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(Toml::as_array)
        .ok_or("Workspace 清单缺少 members")?;
    let mut result = std::collections::BTreeMap::new();
    let mut tables = Vec::new();
    let mut sources = vec![root.join("Cargo.toml")];
    let mut seen = HashSet::new();
    for member in members {
        let member = member.as_str().ok_or("Workspace member 必须是字符串")?;
        let package = safe_member(root, member)?;
        let package_manifest = package.join("Cargo.toml");
        if !package_manifest.is_file() {
            continue;
        }
        let content = read_regular(&package_manifest)?;
        let metadata = toml::from_str::<toml::Table>(
            std::str::from_utf8(&content).map_err(|_| "业务 crate 清单必须是 UTF-8")?,
        )
        .map_err(|error| format!("业务 crate 清单无效：{error}"))?;
        let rykind = metadata
            .get("package")
            .and_then(|value| value.get("metadata"))
            .and_then(|value| value.get("ryframe"));
        let Some(rykind) = rykind else {
            continue;
        };
        if rykind.get("kind").and_then(Toml::as_str) != Some("business") {
            continue;
        }
        sources.push(package_manifest);
        let generated = package.join("src/generated");
        let catalog = generated.join("tenant_catalog.json");
        let mod_file = generated.join("mod.rs");
        if !catalog.exists() {
            let content = owned_text(&package, root, "src/generated/mod.rs")?;
            sources.push(mod_file);
            if !content.contains("pub static RESOURCES: [ryframe_sdk::ResourceDescriptor; 0] = [];")
            {
                return Err("非空业务模块缺少完整租户目录；必须正式重新生成".into());
            }
            continue;
        }
        let catalog_path = owned_path(&package, root, "src/generated/tenant_catalog.json")?;
        let resources_path = owned_path(&package, root, "src/generated/resources.json")?;
        let value = json::read(&catalog_path)?;
        let resources = json::read(&resources_path)?;
        sources.extend([catalog_path, resources_path]);
        validate_module(
            rykind,
            &value,
            &resources,
            &mut result,
            &mut tables,
            &mut seen,
        )?;
    }
    Ok(Loaded {
        columns: result,
        tables,
        sources,
    })
}

fn validate_module(
    marker: &Toml,
    value: &Value,
    resources: &Value,
    result: &mut std::collections::BTreeMap<String, Columns>,
    descriptors: &mut Vec<Value>,
    seen: &mut HashSet<String>,
) -> Result<()> {
    let value = exact(value, &["format_version", "tables"], "业务租户目录")?;
    let resources = object(resources, "业务资源目录")?;
    let module = marker
        .get("module")
        .and_then(Toml::as_str)
        .ok_or("业务 crate 缺少稳定模块标识")?;
    if value.get("format_version").and_then(Value::as_u64) != Some(1)
        || resources.get("format_version").and_then(Value::as_u64) != Some(1)
        || resources.get("module").and_then(Value::as_str) != Some(module)
    {
        return Err("业务机器目录版本、模块归属或完整结构无效".into());
    }
    let resource_items = resources
        .get("resources")
        .ok_or("业务资源目录缺少 resources")?;
    let resource_items = array(resource_items, "业务资源列表")?;
    let expected = resource_items
        .iter()
        .filter_map(|item| {
            let item = item.as_object()?;
            (item.get("database").and_then(Value::as_str) == Some("tenant"))
                .then(|| {
                    item.get("table")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .flatten()
        })
        .collect::<HashSet<_>>();
    let tables = array(
        value.get("tables").ok_or("业务租户目录缺少 tables")?,
        "业务租户表列表",
    )?;
    let actual = tables
        .iter()
        .filter_map(|table| {
            table
                .get("table")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect::<HashSet<_>>();
    if expected != actual {
        return Err("业务资源与租户表目录不同".into());
    }
    for table in tables {
        let (name, columns) = validate_table(table)?;
        if !seen.insert(name.clone()) {
            return Err("多个业务模块重复声明租户表".into());
        }
        result.insert(name, columns);
        descriptors.push(table.clone());
    }
    Ok(())
}

fn validate_table(table: &Value) -> Result<(String, Columns)> {
    let table = exact(
        table,
        &[
            "table",
            "copy_order",
            "tenant_column",
            "primary_key_cursor_columns",
            "checksum_columns",
            "column_types",
            "has_generated_columns",
            "foreign_key_dependencies",
            "foreign_keys",
            "schema_canonical",
        ],
        "业务表描述",
    )?;
    let name = text(table.get("table").ok_or("业务表缺少 table")?, "业务表名")?;
    if !Regex::new(r"^biz_[a-z0-9_]+$")?.is_match(name)
        || table.get("tenant_column").and_then(Value::as_str) != Some("tenant_id")
        || table.get("has_generated_columns").and_then(Value::as_bool) != Some(false)
        || table
            .get("schema_canonical")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err("业务表描述不完整或包含未支持结构".into());
    }
    let copy_order = table.get("copy_order").and_then(Value::as_i64);
    if copy_order.is_none_or(|value| !(1..=i32::MAX as i64).contains(&value)) {
        return Err("业务表复制顺序无效".into());
    }
    let columns = array(
        table
            .get("checksum_columns")
            .ok_or("业务表缺少 checksum_columns")?,
        "业务表列",
    )?;
    let types = array(
        table.get("column_types").ok_or("业务表缺少 column_types")?,
        "业务表类型",
    )?;
    if columns.len() != types.len() || columns.is_empty() {
        return Err("业务表完整列、类型或复制 cursor 无效".into());
    }
    let column_name = Regex::new(r"^[a-z0-9_]+$")?;
    let column_kind = Regex::new(r"^[a-z]+$")?;
    let mut result = Columns::new();
    for (column, kind) in columns.iter().zip(types) {
        let column = text(column, "业务表列名")?;
        let kind = text(kind, "业务表列类型")?;
        if !column_name.is_match(column)
            || !column_kind.is_match(kind)
            || result
                .insert(column.to_owned(), kind.to_ascii_uppercase())
                .is_some()
        {
            return Err("业务表完整列、类型或复制 cursor 无效".into());
        }
    }
    let primary = array(
        table
            .get("primary_key_cursor_columns")
            .ok_or("业务表缺少 primary_key_cursor_columns")?,
        "业务表 cursor",
    )?;
    if primary.is_empty()
        || primary.iter().any(|value| {
            value
                .as_str()
                .is_none_or(|value| !result.contains_key(value))
        })
    {
        return Err("业务表完整列、类型或复制 cursor 无效".into());
    }
    let _ = boolean(
        table
            .get("has_generated_columns")
            .ok_or("业务表缺少 has_generated_columns")?,
        "has_generated_columns",
    )?;
    Ok((name.to_owned(), result))
}

fn owned_path(package: &Path, root: &Path, relative: &str) -> Result<PathBuf> {
    let path = safe_file(package, root, relative)?;
    let manifest = safe_file(package, root, ".ryframe/generated.toml")?;
    let content = read_regular(&manifest)?;
    let document = toml::from_str::<toml::Table>(
        std::str::from_utf8(&content).map_err(|_| "业务生成清单必须是 UTF-8")?,
    )
    .map_err(|error| format!("业务生成清单无效：{error}"))?;
    let entries = document
        .get("entries")
        .and_then(Toml::as_array)
        .ok_or("业务生成清单缺少 entries")?;
    let matching = entries
        .iter()
        .filter(|entry| entry.get("path").and_then(Toml::as_str) == Some(relative))
        .collect::<Vec<_>>();
    let digest = digest::file(&path)?;
    if matching.len() != 1
        || matching[0].get("resource").and_then(Toml::as_str) != Some("__catalog__")
        || matching[0].get("content_hash").and_then(Toml::as_str) != Some(digest.sha256.as_str())
    {
        return Err("业务目录不是当前正式生成入口的受管资产".into());
    }
    Ok(path)
}

fn owned_text(package: &Path, root: &Path, relative: &str) -> Result<String> {
    let path = owned_path(package, root, relative)?;
    String::from_utf8(read_regular(&path)?).map_err(|_| "业务生成文件必须是 UTF-8".into())
}

fn safe_member(root: &Path, member: &str) -> Result<PathBuf> {
    if member.is_empty()
        || Path::new(member).is_absolute()
        || member
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("Workspace 成员越过源码根目录".into());
    }
    let package = root.join(member);
    let canonical_root = fs::canonicalize(root)?;
    let canonical = fs::canonicalize(&package)?;
    if !canonical.starts_with(canonical_root) {
        return Err("Workspace 成员越过源码根目录".into());
    }
    Ok(package)
}

fn safe_file(package: &Path, root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || Path::new(relative).is_absolute()
        || relative
            .split(['/', '\\'])
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("业务目录资产越过已登记 crate".into());
    }
    let path = package.join(relative);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || digest::linked(&metadata) {
        return Err("业务目录资产必须是普通文件且不能经过链接".into());
    }
    let canonical_package = fs::canonicalize(package)?;
    let canonical_root = fs::canonicalize(root)?;
    let canonical = fs::canonicalize(&path)?;
    if !canonical.starts_with(&canonical_package) || !canonical.starts_with(canonical_root) {
        return Err("业务目录资产越过已登记 crate".into());
    }
    Ok(path)
}

fn read_regular(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || digest::linked(&metadata) {
        return Err(format!("受管文件必须是普通文件：{}", path.display()).into());
    }
    Ok(fs::read(path)?)
}

pub(crate) fn ledger_columns(backend: &Path) -> Result<Columns> {
    let path = backend.join("crates/ryframe-sdk/src/migration/ledger.rs");
    digest::file(&path)?;
    let source = fs::read_to_string(path)?;
    let pattern = regex::Regex::new(
        r#"const DDL: &str = "CREATE TABLE ryframe_business_migration \(([^"\n]+)\) ENGINE="#,
    )?;
    let matches = pattern.captures_iter(&source).collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err("正式业务迁移账本 DDL 缺失或不唯一".into());
    }
    let columns = regex::Regex::new(r"(?:^|, )([a-z_]+) (VARCHAR|CHAR|TIMESTAMP|JSON)\b")?;
    let names = columns
        .captures_iter(&matches[0][1])
        .map(|capture| (capture[1].to_owned(), capture[2].to_owned()))
        .collect::<Vec<_>>();
    let result = names.iter().cloned().collect::<Columns>();
    if names.len() != 10 || result.len() != 10 {
        return Err("业务迁移账本完整列结构变化，必须审查复制规则".into());
    }
    Ok(result)
}
