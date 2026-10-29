use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde_json::{Value, json};

use super::super::super::{
    business, catalog, digest,
    input::digest_shape,
    json::{array, exact, text},
};
use super::super::input::request::KEYS;
use crate::Result;

pub(crate) const TABLE_SET_ERROR: &str = "库存完整表集、保留表或迁移账本不同，不能过滤未知表；请核对当前产品目录、逐目标原始库存和工具来源，保留失败证据，仅在正式 status 确认可续作后执行既有库存续作，禁止重放 reset 或迁移";

pub(crate) fn databases(
    context: &super::super::storage::StorageInputs<'_>,
) -> Result<BTreeMap<String, Value>> {
    let entries = context.databases()?;
    let mut result = BTreeMap::new();
    let mut physical = BTreeSet::new();
    for db in entries {
        let key = text(&db["key"], "数据库 key")?;
        let (_, kind, mode) = KEYS
            .iter()
            .find(|(wanted, _, _)| *wanted == key)
            .ok_or("数据库目标未登记")?;
        if db["kind"].as_str() != Some(*kind)
            || db["mode"].as_str() != Some(*mode)
            || !physical.insert(text(&db["database"], "物理库名")?.to_owned())
            || result.insert(key.to_owned(), db).is_some()
        {
            return Err("四目标类型、模式或物理库名重复".into());
        }
    }
    if result.len() != KEYS.len() {
        return Err("库存必须完整覆盖四个逻辑目标".into());
    }
    Ok(result)
}

pub(crate) fn observation(
    backend: &Path,
    value: &Value,
    declared: &Value,
    scope: &str,
    ownership: &[Value],
) -> Result<Value> {
    exact(
        value,
        &[
            "scope_id",
            "control_schema_fingerprint",
            "tenant_schema_fingerprint",
            "target",
        ],
        "逐目标库存",
    )?;
    let target = exact(
        &value["target"],
        &["database", "preserved_tables"],
        "逐目标库存目标",
    )?;
    let db = &target["database"];
    exact(
        db,
        &[
            "key",
            "kind",
            "server_uuid",
            "database",
            "shared",
            "placements",
            "tables",
        ],
        "逐目标库存数据库",
    )?;
    let combined = declared["kind"] == "combined";
    if value["scope_id"].as_str() != Some(scope)
        || ["key", "kind", "server_uuid", "database"]
            .iter()
            .any(|field| db[*field] != declared[*field])
        || db["shared"].as_bool() != Some(declared["mode"] == "shared")
        || ownership != expected_owners(scope, combined)
    {
        return Err("库存 scope、物理目标、共享模式或完整 owner 不匹配".into());
    }
    let (business, expected, raw_preserved) = table_sets(backend, combined)?;
    let tables = table_map(&db["tables"])?;
    let kept = table_map(&target["preserved_tables"])?;
    let names = tables
        .keys()
        .chain(kept.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    if tables.keys().any(|key| kept.contains_key(key))
        || kept.keys().cloned().collect::<BTreeSet<_>>() != raw_preserved
        || names != expected
    {
        return Err(TABLE_SET_ERROR.into());
    }
    let mut all = tables;
    all.extend(kept);
    let preserved = names
        .difference(&business)
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut allowed = catalog::excluded_tables()
        .into_iter()
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    allowed.extend([
        "seaql_tenant_data_migrations".into(),
        "ryframe_business_migration".into(),
    ]);
    if !preserved.is_subset(&allowed)
        || all["ryframe_resource_ownership"]["rows"].as_u64() != Some(if combined { 2 } else { 1 })
    {
        return Err("库存存在未审查保留表或多余 ownership 行".into());
    }
    placements(&db["placements"])?;
    let mut schema = fingerprints(value)?;
    if !combined {
        schema["control_schema_fingerprint"] = Value::Null;
    }
    let select = |keys: &BTreeSet<String>| {
        keys.iter()
            .map(|key| (key.clone(), all[key].clone()))
            .collect::<BTreeMap<_, _>>()
    };
    Ok(json!({
        "resource": {"kind": "database", "scope_id": scope, "server_uuid": db["server_uuid"], "database": db["database"]},
        "schema_sha256": digest::plan_hash(&schema)?, "tables": select(&business),
        "preserved": select(&preserved), "all_tables": names, "ownership": ownership,
    }))
}

type TableSets = (BTreeSet<String>, BTreeSet<String>, BTreeSet<String>);

pub(crate) fn table_sets(backend: &Path, combined: bool) -> Result<TableSets> {
    let loaded = catalog::load(backend)?;
    let mut business = loaded.tenant.keys().cloned().collect::<BTreeSet<_>>();
    let mut raw = BTreeSet::from([
        "ryframe_resource_ownership".into(),
        "seaql_tenant_data_migrations".into(),
        "biz_data_scope_owner".into(),
        "biz_data_scope_dept".into(),
        "biz_data_scope_projection".into(),
    ]);
    if combined {
        business.extend(loaded.control.keys().cloned());
        raw.extend(
            [
                "seaql_migrations",
                "sys_backup_set",
                "sys_backup_resource",
                "sys_restore_run",
            ]
            .map(ToOwned::to_owned),
        );
    }
    if !business::load(backend)?.columns.is_empty() {
        business::ledger_columns(backend)?;
        raw.insert("ryframe_business_migration".into());
    }
    let mut expected = business.union(&raw).cloned().collect::<BTreeSet<_>>();
    if combined {
        let path = backend.join("sql/ryframe_config.sql");
        digest::file(&path)?;
        expected.extend(catalog::parse_schema(&fs::read_to_string(path)?)?.into_keys());
    }
    Ok((business, expected, raw))
}

pub(crate) fn table_map(value: &Value) -> Result<BTreeMap<String, Value>> {
    let mut result = BTreeMap::new();
    for item in array(value, "库存逐表摘要")? {
        let entry = exact(item, &["table", "rows", "sha256"], "库存表")?;
        let name = text(&entry["table"], "库存表名")?;
        if entry["rows"].as_u64().is_none() {
            return Err("库存表行数必须是非负整数".into());
        }
        digest_shape(&entry["sha256"])?;
        if result
            .insert(
                name.into(),
                json!({"rows": entry["rows"], "sha256": entry["sha256"]}),
            )
            .is_some()
        {
            return Err("库存表名重复".into());
        }
    }
    Ok(result)
}

pub(crate) fn placements(value: &Value) -> Result<()> {
    let mut seen = BTreeSet::new();
    for item in array(value, "库存完整租户关系")? {
        let item = exact(
            item,
            &["tenant_id", "generation", "switch_token"],
            "库存租户关系",
        )?;
        let id = text(&item["tenant_id"], "租户 ID")?;
        if id.is_empty()
            || !seen.insert(id)
            || item["generation"]
                .as_u64()
                .filter(|generation| *generation > 0)
                .is_none()
            || text(&item["switch_token"], "租户切换 token")?.is_empty()
        {
            return Err("库存租户关系重复或代次无效".into());
        }
    }
    Ok(())
}

pub(crate) fn fingerprints(value: &Value) -> Result<Value> {
    for (field, length) in [
        ("control_schema_fingerprint", 16),
        ("tenant_schema_fingerprint", 64),
    ] {
        let fingerprint = text(&value[field], "编译 schema 指纹")?;
        if fingerprint.len() != length
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("编译 schema 指纹格式与控制库或租户目录契约不同".into());
        }
    }
    Ok(
        json!({"control_schema_fingerprint": value["control_schema_fingerprint"], "tenant_schema_fingerprint": value["tenant_schema_fingerprint"]}),
    )
}

pub(crate) fn expected_owners(scope: &str, combined: bool) -> Vec<Value> {
    let kinds: &[&str] = if combined {
        &["control", "tenant-data"]
    } else {
        &["tenant-data"]
    };
    kinds.iter().map(|kind| json!({"resource_kind": kind, "scope_id": scope, "marker": format!("ryframe-owner:v1:{scope}:{kind}")})).collect()
}

pub(crate) fn owners(response: &str) -> Result<Vec<Value>> {
    response
        .lines()
        .map(|line| {
            let parts = line.split('\t').collect::<Vec<_>>();
            if parts.len() != 3 {
                return Err("目标 ownership 行格式无效".into());
            }
            Ok(json!({"resource_kind": parts[0], "scope_id": parts[1], "marker": parts[2]}))
        })
        .collect()
}

pub(crate) fn physical_images(
    context: &super::super::storage::StorageInputs<'_>,
    logical: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, Value>> {
    let mut result = BTreeMap::new();
    for (key, db) in databases(context)? {
        let image = logical.get(&key).ok_or("库存缺少逻辑目标观察")?;
        fresh(image, db["kind"] == "combined")?;
        result.insert(text(&db["database"], "物理库名")?.into(), image.clone());
    }
    Ok(result)
}

pub(crate) fn fresh(image: &Value, combined: bool) -> Result<()> {
    let tables = image["tables"].as_object().ok_or("库存缺少业务表")?;
    for (name, value) in tables {
        if name.starts_with("biz_")
            && !matches!(name.as_str(), "biz_tenant_fence" | "biz_tenant_target_slot")
            && value["rows"].as_u64() != Some(0)
        {
            return Err("fresh 目标已出现业务数据".into());
        }
    }
    if !combined
        && (image["tables"]["biz_tenant_fence"]["rows"].as_u64() != Some(0)
            || image["tables"]["biz_tenant_target_slot"]["rows"].as_u64() != Some(1))
    {
        return Err("外部新租户库 fence 或空槽行不符".into());
    }
    Ok(())
}
