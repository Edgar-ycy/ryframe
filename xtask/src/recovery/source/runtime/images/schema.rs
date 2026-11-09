//! 复用正式 schema 目录与逐目标库存合成；完整像不能过滤未知表或省略空目标。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        catalog::{self, Catalog, OrderedCatalog},
        digest,
        export::{database::Models, inventory},
        json as document,
        resources::common,
    },
};

pub(super) fn models(execution: &Path) -> Result<Models> {
    let loaded = catalog::load_ordered(execution, true)?;
    let (full_control, full_tenant) = loaded.full.ok_or("完整像 schema 缺少正式全目录")?;
    let unordered = |value: OrderedCatalog| -> Catalog {
        value
            .into_iter()
            .map(|(name, columns)| (name, columns.into_iter().collect()))
            .collect()
    };
    Ok(Models {
        control: unordered(loaded.control),
        tenant: unordered(loaded.tenant),
        full_control: unordered(full_control),
        full_tenant: unordered(full_tenant),
    })
}

pub(super) fn verify(value: &Value, selected: &Value, models: &Models) -> Result<()> {
    let declared = document::array(&selected["databases"], "完整像四库")?;
    let entries = document::array(value, "完整像 schema")?;
    let mut seen = BTreeSet::new();
    if entries.len() != declared.len() {
        return Err("源 schema 必须完整覆盖所有登记数据库".into());
    }
    for row in entries {
        document::exact(row, &["key", "columns", "sha256"], "源 schema")?;
        let key = document::text(&row["key"], "源 schema 目标")?;
        let db = declared
            .iter()
            .find(|db| db["key"] == key)
            .ok_or("schema 含未登记数据库")?;
        if !seen.insert(key) {
            return Err("源 schema 数据库重复".into());
        }
        let mut known = if db["kind"] == "combined" {
            models.full_control.clone()
        } else {
            Catalog::new()
        };
        known.extend(models.full_tenant.clone());
        let columns = document::object(&row["columns"], "源 schema 列集合")?;
        let mut names = known.keys().map(String::as_str).collect::<BTreeSet<_>>();
        names.insert("seaql_tenant_data_migrations");
        if db["kind"] == "combined" {
            names.insert("seaql_migrations");
        }
        if names != columns.keys().map(String::as_str).collect()
            || known
                .iter()
                .any(|(name, fields)| row["columns"][name] != json!(fields))
            || columns.values().any(|fields| {
                fields.as_object().is_none_or(|fields| {
                    fields.is_empty() || fields.values().any(|value| !value.is_string())
                })
            })
            || row["sha256"] != digest::plan_hash(&row["columns"])?
        {
            return Err("源 schema 与当前完整 catalog 或原摘要不同".into());
        }
    }
    Ok(())
}

pub(super) fn origins(
    backend: &Path,
    directory: &Path,
    selected: &Value,
    phase: &str,
    logical: &Value,
) -> Result<()> {
    let manifest = document::read(&directory.join(format!("inventory-{phase}-targets.json")))?;
    document::exact(&manifest, &["runtime", "targets"], "逻辑库存原始绑定")?;
    let runtime_name = format!("inventory-{phase}-runtime.json");
    let runtime = read_relative(backend, directory, &manifest["runtime"], &runtime_name)?;
    let targets = document::array(&manifest["targets"], "逻辑库存目标原始绑定")?;
    let expected = document::array(&selected["databases"], "来源四库")?
        .iter()
        .map(|db| {
            Ok(format!(
                "inventory-{phase}-target-{}.json",
                document::text(&db["key"], "来源数据库 key")?
            ))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let mut raw = BTreeMap::new();
    for descriptor in targets {
        let name = document::text(&descriptor["file"], "逐目标库存文件")?;
        if !expected.contains(name) || raw.contains_key(name) {
            return Err("逐目标原始文件未精确覆盖登记目标".into());
        }
        let value = read_relative(backend, directory, descriptor, name)?;
        if name
            != format!(
                "inventory-{phase}-target-{}.json",
                document::text(&value["target"]["database"]["key"], "原始目标 key")?
            )
        {
            return Err("逐目标原始文件名与实际目标不同".into());
        }
        raw.insert(name.to_owned(), value);
    }
    if raw.keys().cloned().collect::<BTreeSet<_>>() != expected
        || inventory::complete_selected(&runtime, raw.into_values().collect(), selected)?
            != *logical
    {
        return Err("完整库存与已绑定的原始逐目标库存不同".into());
    }
    Ok(())
}

fn read_relative(
    backend: &Path,
    directory: &Path,
    descriptor: &Value,
    name: &str,
) -> Result<Value> {
    document::exact(
        descriptor,
        &["file", "bytes", "sha256"],
        "逻辑库存相对文件绑定",
    )?;
    if descriptor["file"] != name {
        return Err("逻辑库存原始文件名不符".into());
    }
    let path = directory.join(name);
    common::read_bound(
        backend,
        &json!({"path":path,"bytes":descriptor["bytes"],"sha256":descriptor["sha256"]}),
        "逻辑库存原始输出",
    )
}
