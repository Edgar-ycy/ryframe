use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::{Models, SavedObjectInputs, digest, document, inventory, sql};
use crate::{
    Result,
    recovery::clone::native::{catalog::Catalog, source},
};

pub(super) fn verify(
    inputs: &SavedObjectInputs,
    evidence: &BTreeMap<&str, Value>,
    models: &Models,
) -> Result<Value> {
    let export = &inputs.export;
    let request = &inputs.request;
    let source = &request["source"];
    let generation = &evidence["generation-before"];
    if evidence["generation-after"] != *generation
        || export["source_snapshot"] != generation["source"]
        || generation["worktree_fingerprint"] != export["worktree_fingerprint"]
        || request["worktree_fingerprint"] != export["worktree_fingerprint"]
        || generation["request_sha256"] != digest::plan_hash(request)?
        || generation["maintenance"]["source"]["snapshot"] != generation["source"]
        || generation["maintenance"]["source"]["worktree_fingerprint"]
            != export["worktree_fingerprint"]
        || export["catalog_sha256"] != models.fingerprint()?
    {
        return Err("源前后代次、构建来源或实际 catalog 不一致".into());
    }
    let head = document::text(&generation["source"]["head"], "源构建提交")?;
    let stopped = document::text(&export["quiesced_at"], "源停止时间")?;
    let declared = databases(source)?;
    for phase in ["before", "after"] {
        inventory::validate_selected(
            &evidence[format!("inventory-{phase}").as_str()],
            source,
            models,
            head,
            stopped,
            false,
        )?;
        schema(
            &evidence[format!("schema-{phase}").as_str()],
            &declared,
            models,
        )?;
    }
    let logical = inventory::logical(&evidence["inventory-before"])?;
    if evidence["inventory-before"] != inputs.inventory
        || logical != inventory::logical(&evidence["inventory-after"])?
        || export["logical_inventory_sha256"] != digest::plan_hash(&logical)?
        || evidence["schema-before"] != evidence["schema-after"]
    {
        return Err("源前后完整逻辑清单或 schema 不一致".into());
    }
    Ok(generation.clone())
}

pub(super) fn receipts(
    generation: &Value,
    build: &Value,
    maintenance: &Value,
    runtime: &Value,
    api: &Value,
    worker: &Value,
) -> Result<()> {
    if generation.get("source") != Some(build_snapshot(build)?)
        || generation.get("maintenance") != Some(maintenance)
        || generation.get("runtime") != Some(runtime)
        || api
            .get("identity")
            .is_none_or(|identity| generation["processes"].get("api") != Some(identity))
        || worker
            .get("identity")
            .is_none_or(|identity| generation["processes"].get("worker") != Some(identity))
    {
        return Err("源记录代次与已绑定构建、运行或进程收据不同".into());
    }
    Ok(())
}

fn build_snapshot(value: &Value) -> Result<&Value> {
    let binding = if value["kind"] == "devex-clone-tool-build" {
        &value["source"]
    } else {
        document::exact(
            value,
            &["format_version", "kind", "sources", "build", "artifacts"],
            "源后端构建",
        )?;
        if value["kind"] != "restore-backend-build" || value["format_version"] != 2 {
            return Err("源后端构建收据类型或版本无效".into());
        }
        source::protocol::request(
            &json!({"action":"validate-domains","repository":"backend","value":value["sources"]}),
        )?;
        &value["sources"]["full"]["source"]
    };
    binding
        .get("snapshot")
        .ok_or_else(|| "源构建缺少快照".into())
}

fn schema(value: &Value, declared: &BTreeMap<&str, &Value>, models: &Models) -> Result<()> {
    document::exact(value, &["databases"], "保存的源 schema")?;
    let rows = document::array(&value["databases"], "保存的源 schema 数据库")?;
    coverage(rows, declared, "源 schema 必须完整覆盖所有登记数据库")?;
    for row in rows {
        document::exact(row, &["key", "columns", "sha256"], "保存的源 schema 数据库")?;
        let key = document::text(&row["key"], "源 schema 逻辑库")?;
        let combined = declared[key]["kind"] == "combined";
        let known = sql::all_columns(&models.full_control, &models.full_tenant, combined);
        schema_columns(&row["columns"], &known, combined)?;
        if row["sha256"] != digest::plan_hash(&row["columns"])? {
            return Err("源 schema 列清单与原摘要不同".into());
        }
    }
    Ok(())
}

fn schema_columns(value: &Value, known: &Catalog, combined: bool) -> Result<()> {
    let actual = document::object(value, "源 schema 列清单")?;
    let mut expected: BTreeSet<_> = known.keys().map(String::as_str).collect();
    expected.insert("seaql_tenant_data_migrations");
    if combined {
        expected.insert("seaql_migrations");
    }
    if actual.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected
        || known
            .iter()
            .any(|(table, fields)| actual[table] != json!(fields))
        || actual.values().any(|fields| {
            fields.as_object().is_none_or(|fields| {
                fields.is_empty() || fields.values().any(|value| !value.is_string())
            })
        })
    {
        return Err("源 schema 与完整 catalog 或迁移账本表集不同".into());
    }
    Ok(())
}

pub(super) fn dumps<'a>(
    source: &Value,
    dumps: &'a [Value],
    inventory: &Value,
    models: &Models,
) -> Result<Vec<&'a Value>> {
    let declared = databases(source)?;
    coverage(dumps, &declared, "转储文件没有精确覆盖全部数据库")?;
    let baseline = document::array(&inventory["databases"], "源库存数据库")?
        .iter()
        .map(|row| Ok((document::text(&row["key"], "库存逻辑库")?, row)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    dumps
        .iter()
        .map(|dump| {
            document::exact(dump, &["key", "tables", "artifact"], "源数据库转储")?;
            let key = document::text(&dump["key"], "源转储逻辑库")?;
            let catalog = sql::all_columns(
                &models.control,
                &models.tenant,
                declared[key]["kind"] == "combined",
            );
            let actual = document::object(&dump["tables"], "源转储表行数")?;
            let inventory = baseline.get(key).ok_or("转储逻辑库缺少原库存")?;
            dump_tables(actual, &inventory["tables"], &catalog)?;
            Ok(&dump["artifact"])
        })
        .collect()
}

fn dump_tables(
    actual: &serde_json::Map<String, Value>,
    inventory: &Value,
    catalog: &Catalog,
) -> Result<()> {
    let expected = document::array(inventory, "源库存表")?
        .iter()
        .map(|row| Ok((document::text(&row["table"], "库存表名")?, &row["rows"])))
        .collect::<Result<BTreeMap<_, _>>>()?;
    if actual.keys().ne(catalog.keys())
        || actual.get("biz_tenant_target_slot").and_then(Value::as_u64) != Some(1)
        || actual.iter().any(|(table, count)| {
            count.as_u64().is_none() || expected.get(table.as_str()) != Some(&count)
        })
    {
        return Err("转储行数、固定槽位或含空表的完整表集与源 inventory 不同".into());
    }
    Ok(())
}

fn databases(source: &Value) -> Result<BTreeMap<&str, &Value>> {
    document::array(&source["databases"], "登记源数据库")?
        .iter()
        .map(|row| Ok((document::text(&row["key"], "源数据库键")?, row)))
        .collect()
}

pub(super) fn tenant_identifiers(tenants: &BTreeSet<String>, source: &Value) -> Result<()> {
    let scope = document::text(&source["scope_id"], "来源 scope")?;
    if tenants.contains(scope) {
        return Err("来源逻辑租户与物理 scope 重名".into());
    }
    for database in document::array(&source["databases"], "登记源数据库")? {
        let physical = document::text(&database["database"], "物理数据库名")?;
        if tenants.contains(physical) {
            return Err("来源逻辑租户与物理数据库重名".into());
        }
    }
    Ok(())
}

fn coverage(rows: &[Value], declared: &BTreeMap<&str, &Value>, error: &'static str) -> Result<()> {
    let actual = rows
        .iter()
        .map(|row| document::text(&row["key"], "源数据库键"))
        .collect::<Result<BTreeSet<_>>>()?;
    if rows.len() != declared.len() || actual.into_iter().ne(declared.keys().copied()) {
        return Err(error.into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../../tests/internal/export_native/frozen.rs"]
mod tests;
