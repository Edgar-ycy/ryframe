//! 从已核验的复制计划和独立观察生成账本使用的纯前后像。

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::super::{
    catalog,
    copy_ledger::model,
    digest,
    input::{BUCKETS, digest_shape, name},
    json as document, metadata,
    s3::protocol::validate_key,
    target_verify::database::model::expected_owners,
};
use crate::{
    Result,
    recovery::inputs::{document::binding_shape, domain},
};

pub(crate) struct DatabaseSide<'a> {
    pub(crate) scope: &'a str,
    pub(crate) declaration: &'a Value,
    pub(crate) preserved_tables: &'a BTreeSet<String>,
}

pub(crate) struct DatabaseBasis<'a> {
    pub(crate) source: &'a Value,
    pub(crate) initialized: &'a Value,
    pub(crate) artifact_sha256: &'a str,
}

pub(crate) fn database_pair(
    generation: &str,
    item: &Value,
    source: DatabaseSide<'_>,
    target: DatabaseSide<'_>,
    basis: DatabaseBasis<'_>,
) -> Result<(Value, Value)> {
    let counts = database_plan(item)?;
    let artifact = artifact(&item["artifact"])?;
    if artifact["sha256"].as_str() != Some(basis.artifact_sha256) || source.scope == target.scope {
        return Err("数据库导出前后像与计划 SQL 或异侧 scope 不匹配".into());
    }
    let source = database_image(
        generation,
        item,
        "source_database",
        &source,
        basis.source,
        counts,
    )?;
    let before = database_image(
        generation,
        item,
        "target_database",
        &target,
        basis.initialized,
        counts,
    )?;
    for (table, count) in counts {
        if &basis.source["tables"][table.as_str()]["rows"] != count {
            return Err("源完整观察行数不匹配离线 SQL".into());
        }
    }
    let mut after = before.clone();
    after["tables_sha256"] = source["tables_sha256"].clone();
    // 相同的完整数据库像由上层记录为已核验的无写入步骤。
    Ok((before, model::image(&after)?))
}

pub(crate) fn database_observed(
    generation: &str,
    item: &Value,
    target: DatabaseSide<'_>,
    observed: &Value,
) -> Result<Value> {
    let counts = database_plan(item)?;
    database_image(
        generation,
        item,
        "target_database",
        &target,
        observed,
        counts,
    )
}

fn database_plan(item: &Value) -> Result<&Map<String, Value>> {
    document::exact(
        item,
        &[
            "key",
            "source_database",
            "target_database",
            "tables",
            "artifact",
            "preserve_target_ownership",
        ],
        "复制数据库计划",
    )?;
    let counts = document::object(&item["tables"], "复制计划完整业务表集")?;
    if item["preserve_target_ownership"].as_bool() != Some(true)
        || counts.values().any(|count| count.as_u64().is_none())
    {
        return Err("复制数据库必须保留目标 ownership 并登记完整非负行数".into());
    }
    Ok(counts)
}

fn database_image(
    generation: &str,
    item: &Value,
    side_key: &str,
    side: &DatabaseSide<'_>,
    observed: &Value,
    counts: &Map<String, Value>,
) -> Result<Value> {
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
        "数据库完整观察",
    )?;
    let declared = side.declaration;
    let resource = json!({
        "kind": "database", "scope_id": name(side.scope)?,
        "server_uuid": declared["server_uuid"], "database": declared["database"],
    });
    let schema = digest_shape(&declared["schema_sha256"])?;
    if declared["key"] != item["key"]
        || declared["database"] != item[side_key]
        || observed["resource"] != resource
        || observed["schema_sha256"] != schema
    {
        return Err("数据库观察物理身份、scope 或 schema 与本侧计划不同".into());
    }
    verify_tables(observed, counts, side.preserved_tables)?;
    let combined = declared["kind"] == "combined";
    let expected = json!(expected_owners(side.scope, combined));
    let owners = sorted_owners(&expected)?;
    if owners != sorted_owners(&declared["ownership"])?
        || owners != sorted_owners(&observed["ownership"])?
    {
        return Err("数据库观察未保留本侧的精确 ownership".into());
    }
    model::image(&json!({
        "kind": "database", "resource_sha256": resource_sha256(generation, &resource)?,
        "schema_sha256": schema, "tables_sha256": digest::plan_hash(&observed["tables"])?,
        "preserved_sha256": digest::plan_hash(&observed["preserved"])?,
    }))
}

fn sorted_owners(value: &Value) -> Result<Vec<(String, &Value)>> {
    let mut owners = document::array(value, "数据库观察 ownership")?
        .iter()
        .map(|entry| Ok((digest::plan_hash(entry)?, entry)))
        .collect::<Result<Vec<_>>>()?;
    owners.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    Ok(owners)
}

fn verify_tables(
    observed: &Value,
    counts: &Map<String, Value>,
    expected_preserved: &BTreeSet<String>,
) -> Result<()> {
    let tables = table_images(&observed["tables"])?;
    let preserved = table_images(&observed["preserved"])?;
    let planned = counts.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let kept = expected_preserved
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut allowed = catalog::excluded_tables()
        .into_iter()
        .collect::<BTreeSet<_>>();
    allowed.extend(["seaql_tenant_data_migrations", "ryframe_business_migration"]);
    let all = document::array(&observed["all_tables"], "数据库观察完整表集")?;
    let observed_names = all
        .iter()
        .map(|name| document::text(name, "数据库观察表名"))
        .collect::<Result<BTreeSet<_>>>()?;
    if tables != planned
        || preserved != kept
        || !tables.is_disjoint(&allowed)
        || !preserved.is_subset(&allowed)
        || !preserved.contains("ryframe_resource_ownership")
        || observed_names.len() != all.len()
        || observed_names != tables.union(&preserved).copied().collect::<BTreeSet<_>>()
    {
        return Err("数据库观察完整业务表集、保留表集或物理表集与计划不同".into());
    }
    Ok(())
}

fn table_images(value: &Value) -> Result<BTreeSet<&str>> {
    let values = document::object(value, "数据库逐表完整行像")?;
    for entry in values.values() {
        document::exact(entry, &["rows", "sha256"], "数据库逐表完整行像")?;
        if entry["rows"].as_u64().is_none() {
            return Err("数据库逐表观察缺少非负整数行数".into());
        }
        digest_shape(&entry["sha256"])?;
    }
    Ok(values.keys().map(String::as_str).collect())
}

pub(crate) struct TargetObject<'a> {
    pub(crate) scope: &'a str,
    pub(crate) endpoint: &'a str,
    pub(crate) item: &'a Value,
}

/// 调用方先核验独立 404 证据或完整采集链，失败诊断不得作为这两种证据传入。
pub(crate) enum ObjectObservation<'a> {
    Absent {
        receipt: &'a Value,
        descriptor: &'a Value,
        raw_sha256: &'a str,
    },
    Captured(&'a Value),
}

pub(crate) fn object_pair(generation: &str, target: TargetObject<'_>) -> Result<(Value, Value)> {
    let resource = target_resource(&target)?;
    let resource_sha = resource_sha256(generation, &resource)?;
    let expected = artifact(&target.item["artifact"])?;
    let stored = canonical_metadata(&target.item["metadata"])?;
    let before = absent_image(&resource_sha)?;
    let after = present_image(&resource_sha, &expected, &stored)?;
    model::image_pair(&before, &after)?;
    Ok((before, after))
}

pub(crate) fn object_observed(
    generation: &str,
    target: TargetObject<'_>,
    observed: ObjectObservation<'_>,
) -> Result<Value> {
    let resource = target_resource(&target)?;
    let resource_sha = resource_sha256(generation, &resource)?;
    let expected = artifact(&target.item["artifact"])?;
    match observed {
        ObjectObservation::Absent {
            receipt,
            descriptor,
            raw_sha256,
        } => {
            let descriptor = binding_shape(descriptor, "目标对象缺失原始收据")?;
            if descriptor.sha256 != raw_sha256
                || receipt["format_version"].as_u64() != Some(1)
                || receipt["status"] != "object_absent"
                || receipt["resource"] != resource
                || receipt.get("capture") != Some(&Value::Null)
                || receipt["remote_writes"].as_u64() != Some(0)
            {
                return Err("没有当前目标对象的明确缺失证据".into());
            }
            absent_image(&resource_sha)
        }
        ObjectObservation::Captured(capture) => {
            verify_capture(capture, &resource, &expected)?;
            let stored = canonical_metadata(&capture["metadata"])?;
            let differences = header_differences(capture, &stored)?;
            let image_metadata = if differences.is_empty() {
                stored
            } else {
                json!({"stored":stored,"get_header_differences":differences})
            };
            present_image(&resource_sha, &expected, &image_metadata)
        }
    }
}

fn target_resource(target: &TargetObject<'_>) -> Result<Value> {
    document::exact(
        target.item,
        &["bucket", "source_key", "target_key", "artifact", "metadata"],
        "复制对象计划",
    )?;
    let scope = name(target.scope)?;
    domain::endpoint(&json!(target.endpoint), "复制目标对象端点", None)?;
    let bucket = document::text(&target.item["bucket"], "复制对象桶")?;
    let key = document::text(&target.item["target_key"], "复制目标对象键")?;
    if !BUCKETS.contains(&bucket) {
        return Err("复制目标对象桶不在登记范围".into());
    }
    validate_key(scope, key, false)?;
    Ok(
        json!({"kind":"object","scope_id":scope,"endpoint":target.endpoint,"bucket":bucket,"key":key}),
    )
}

fn verify_capture(capture: &Value, resource: &Value, expected: &Value) -> Result<()> {
    let actual = json!({"file":"object.bin","bytes":expected["bytes"],"sha256":expected["sha256"]});
    if capture["format_version"].as_u64() != Some(1)
        || capture["kind"] != "devex-clone-object-capture"
        || capture["status"] != "captured"
        || capture["side"] != "target"
        || capture["transport"] != "rust-s3-sigv4"
        || capture["clone_verified"].as_bool() != Some(false)
        || capture["stored_metadata_verified"].as_bool() != Some(true)
        || capture["bytes_verified"].as_bool() != Some(true)
        || ["scope_id", "endpoint", "bucket", "key"]
            .iter()
            .any(|field| capture[*field] != resource[*field])
        || capture.get("absence_evidence_sha256").is_some()
        || &capture["expected"] != expected
        || capture["artifact"] != actual
        || capture["max_bytes"]
            .as_u64()
            .zip(expected["bytes"].as_u64())
            .is_none_or(|(max, bytes)| bytes > max)
    {
        return Err("目标对象完整采集与本计划资源或 artifact 不同".into());
    }
    Ok(())
}

fn header_differences<'a>(capture: &'a Value, stored: &Value) -> Result<&'a [Value]> {
    let differences = document::array(&capture["get_header_differences"], "GET 响应头差异")?;
    if capture["get_header_consistent"].as_bool() != Some(differences.is_empty()) {
        return Err("对象 GET 响应头差异状态不一致".into());
    }
    let mut seen = BTreeSet::new();
    for entry in differences {
        document::exact(
            entry,
            &[
                "field",
                "head_present",
                "head_value",
                "get_present",
                "get_value",
            ],
            "GET 响应头差异",
        )?;
        let field = document::text(&entry["field"], "对象 GET 差异字段")?;
        let head = document::boolean(&entry["head_present"], "对象 HEAD 字段存在性")?;
        let get = document::boolean(&entry["get_present"], "对象 GET 字段存在性")?;
        if stored.get(field).is_none()
            || !seen.insert(field)
            || (!head && !entry["head_value"].is_null())
            || (!get && !entry["get_value"].is_null())
            || (head == get && entry["head_value"] == entry["get_value"])
        {
            return Err("对象 GET 响应头差异不属于精确采集".into());
        }
    }
    Ok(differences)
}

fn artifact(value: &Value) -> Result<Value> {
    document::exact(value, &["file", "bytes", "sha256"], "复制计划 artifact")?;
    let file = document::text(&value["file"], "复制计划 artifact 文件名")?;
    let sha = digest_shape(&value["sha256"])?;
    let bytes = value["bytes"]
        .as_u64()
        .ok_or("复制计划 artifact 缺少非负整数大小")?;
    if file.is_empty() {
        return Err("复制计划 artifact 缺少文件名".into());
    }
    Ok(json!({"bytes":bytes,"sha256":sha}))
}

fn canonical_metadata(value: &Value) -> Result<Value> {
    let stored = metadata::copy(value)?;
    if &stored != value {
        return Err("对象 metadata 不是已核验的规范值".into());
    }
    Ok(stored)
}

fn resource_sha256(generation: &str, resource: &Value) -> Result<String> {
    digest_shape(&json!(generation))?;
    let mut identity = resource.clone();
    identity["generation_sha256"] = json!(generation);
    digest::plan_hash(&identity)
}

fn absent_image(resource_sha256: &str) -> Result<Value> {
    model::image(&json!({"kind":"object","resource_sha256":resource_sha256,
        "exists":false,"bytes":null,"sha256":null,"metadata_sha256":null}))
}

fn present_image(resource_sha256: &str, artifact: &Value, metadata: &Value) -> Result<Value> {
    model::image(&json!({"kind":"object","resource_sha256":resource_sha256,
        "exists":true,"bytes":artifact["bytes"],"sha256":artifact["sha256"],
        "metadata_sha256":digest::plan_hash(metadata)?}))
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/copy_transfer/images.rs"]
mod tests;
