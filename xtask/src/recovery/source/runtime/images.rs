//! 运行中来源的四库、五桶完整像；只读核验保存事实，不补造实时资格。
use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    recovery::clone::native::{
        digest,
        export::inventory as logical_inventory,
        input::BUCKETS,
        json as document,
        resources::common,
        target_verify::{guard::Inputs, tree},
    },
};

#[path = "images/capture.rs"]
pub(super) mod capture;

#[path = "images/inventory.rs"]
pub(super) mod inventory;
#[path = "images/objects.rs"]
mod objects;
#[path = "images/schema.rs"]
mod schema;

const FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "inventory",
    "raw_inventories",
    "object_evidence",
    "owner_evidence",
    "logical_inventories",
    "inventory_origins",
    "schema_evidence",
    "image",
    "image_sha256",
    "scale",
    "source_registration",
];

pub(super) fn verify(
    backend: &Path,
    descriptor: &Value,
    selected: &Value,
    predecessor: &Value,
    registration: &Value,
) -> Result<Value> {
    let path = common::bound_file(backend, descriptor, "来源完整像")?;
    let directory = path.parent().ok_or("来源完整像缺少目录")?;
    let before = tree::snapshot(backend, directory, false)?;
    let held = Inputs::open(tree::files(directory, &before)?)?;
    let value = common::read_bound(backend, descriptor, "来源完整像")?;
    document::exact(&value, FIELDS, "来源完整像")?;
    document::exact(
        &value["image"],
        &[
            "databases",
            "schema",
            "objects",
            "owners",
            "redis",
            "storage",
        ],
        "来源逻辑像",
    )?;
    document::exact(
        &value["image"]["databases"],
        &["shared-control", "shared", "dedicated-a", "dedicated-b"],
        "来源四库像",
    )?;
    document::exact(&value["image"]["objects"], BUCKETS, "来源五桶像")?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != "seed-source-generation-image"
        || value["image_sha256"] != digest::plan_hash(&value["image"])?
        || value["scale"] != scale(&value["image"])?
        || value["source_registration"] != *registration
    {
        return Err("source-generation 完整像类型、来源登记或摘要不同".into());
    }
    common::bound_file(backend, registration, "来源原登记结果")?;
    let runtime = document::read(
        &Path::new(document::text(&selected["runtime_dir"], "来源 runtime")?).join("runtime.json"),
    )?;
    let execution = Path::new(document::text(&runtime["backend_root"], "来源执行后端")?);
    let models = schema::models(execution)?;
    schema::verify(&value["image"]["schema"], selected, &models)?;
    verify_schema_files(backend, directory, &value)?;
    if common::bound_file(backend, &value["inventory"], "完整四库收据")?
        != directory.join("databases/inventory.json")
    {
        return Err("完整库存必须属于同一像目录".into());
    }
    let databases = inventory::verify(backend, &value["inventory"], selected, execution)?;
    verify_raw(backend, directory, &value, &databases)?;
    verify_owners(backend, directory, &value, selected)?;
    verify_objects(backend, directory, &value, selected, predecessor)?;
    verify_logical(backend, directory, &value, selected, &models, &databases)?;
    held.check()?;
    if tree::snapshot(backend, directory, false)? != before {
        return Err("来源完整像核验期间原始证据、目录或文件集合变化".into());
    }
    common::bound_file(backend, descriptor, "来源完整像")?;
    Ok(value)
}

fn verify_schema_files(backend: &Path, directory: &Path, value: &Value) -> Result<()> {
    let mut expected = Vec::new();
    for phase in ["before", "after"] {
        for item in document::array(&value["image"]["schema"], "来源 schema")? {
            let path = directory.join(format!(
                "schema-{phase}-{}.json",
                document::text(&item["key"], "schema key")?
            ));
            let descriptor = common::binding(&path)?;
            if common::read_bound(backend, &descriptor, "schema 原始证据")?
                != json!({"key":item["key"],"columns":item["columns"]})
            {
                return Err("source-generation schema 与完整原始像不同".into());
            }
            expected.push(descriptor);
        }
    }
    if value["schema_evidence"] != json!(expected) {
        return Err("source-generation schema 原始证据集合不同".into());
    }
    Ok(())
}

fn verify_raw(backend: &Path, directory: &Path, value: &Value, verified: &Value) -> Result<()> {
    let mut expected = Vec::new();
    for phase in ["before", "after"] {
        for key in ["shared-control", "shared", "dedicated-a", "dedicated-b"] {
            let descriptor =
                common::binding(&directory.join(format!("databases/{phase}-target-{key}.json")))?;
            let raw = common::read_bound(backend, &descriptor, "四库原始输出")?;
            if raw != value["image"]["databases"][key] || raw != verified["databases"][key] {
                return Err("source-generation 完整数据库像与八份原始库存不同".into());
            }
            expected.push(descriptor);
        }
    }
    if value["raw_inventories"] != json!(expected) {
        return Err("完整数据库像没有精确绑定八份原始输出".into());
    }
    Ok(())
}

fn verify_owners(backend: &Path, directory: &Path, value: &Value, selected: &Value) -> Result<()> {
    let scope = document::text(&selected["scope_id"], "对象 scope")?;
    let mut buckets = BUCKETS.to_vec();
    buckets.sort_unstable();
    let mut expected = Vec::new();
    for phase in ["before", "after"] {
        let mut owners = Vec::new();
        for bucket in &buckets {
            let owner = objects::owner(
                backend,
                directory,
                &format!("owners-{phase}"),
                bucket,
                scope,
            )?;
            expected.push(common::binding(
                &directory.join(document::text(&owner["file"], "ownership 文件")?),
            )?);
            owners.push(json!({"bucket":owner["bucket"],"key":owner["key"],"bytes":owner["bytes"],"sha256":owner["sha256"]}));
        }
        if value["image"]["owners"] != json!(owners) {
            return Err("完整像必须绑定同一 scope 的完整五桶 ownership".into());
        }
    }
    if value["owner_evidence"] != json!(expected) {
        return Err("source-generation ownership 原始证据集合不同".into());
    }
    Ok(())
}

fn verify_objects(
    backend: &Path,
    directory: &Path,
    value: &Value,
    selected: &Value,
    predecessor: &Value,
) -> Result<()> {
    let mut observed = BUCKETS
        .iter()
        .map(|bucket| ((*bucket).to_owned(), BTreeMap::<String, Value>::new()))
        .collect::<BTreeMap<_, _>>();
    let max = predecessor["max_object_bytes"]
        .as_u64()
        .filter(|bytes| *bytes > 0)
        .ok_or("来源对象大小上限无效")?;
    for descriptor in document::array(&value["object_evidence"], "完整对象证据")? {
        let path = common::bound_file(backend, descriptor, "完整对象采集证据")?;
        let raw = common::read_bound(backend, descriptor, "完整对象采集证据")?;
        let bucket = document::text(&raw["bucket"], "对象桶")?;
        let key = document::text(&raw["key"], "对象键")?;
        let token = hex::encode(Sha256::digest(format!("{bucket}:{key}").as_bytes()));
        if path != directory.join("objects").join(token).join("capture.json") {
            return Err("对象证据必须属于同一像与明确对象目录".into());
        }
        let expected = &value["image"]["objects"][bucket][key];
        document::exact(
            expected,
            &["bytes", "sha256", "metadata", "identity"],
            "完整对象像",
        )?;
        let capture = objects::verify(
            backend,
            selected,
            path.parent().ok_or("对象证据缺少目录")?,
            &json!({"bytes":expected["bytes"],"sha256":expected["sha256"]}),
            max,
        )?;
        let rows = observed.get_mut(bucket).ok_or("完整像对象包含未知桶")?;
        if rows
            .insert(
                key.to_owned(),
                json!({"bytes":capture["artifact"]["bytes"],"sha256":capture["artifact"]["sha256"],
            "metadata":capture["metadata"],"identity":capture["identity"]}),
            )
            .is_some()
        {
            return Err("source-generation 对象证据重复".into());
        }
    }
    if json!(observed) != value["image"]["objects"] {
        return Err("对象完整字节或存储元数据与原始采集不符".into());
    }
    Ok(())
}

fn verify_logical(
    backend: &Path,
    directory: &Path,
    value: &Value,
    selected: &Value,
    models: &crate::recovery::clone::native::export::database::Models,
    databases: &Value,
) -> Result<()> {
    let expected = ["before", "after"]
        .map(|phase| common::binding(&directory.join(format!("inventory-{phase}.json"))));
    if value["logical_inventories"] != json!(expected.into_iter().collect::<Result<Vec<_>>>()?) {
        return Err("运行中完整像缺少同目录两次观察库存".into());
    }
    let expected = ["before", "after"]
        .map(|phase| common::binding(&directory.join(format!("inventory-{phase}-targets.json"))));
    if value["inventory_origins"] != json!(expected.into_iter().collect::<Result<Vec<_>>>()?) {
        return Err("运行中完整像缺少原始逐目标库存绑定".into());
    }
    let mut previous = None;
    let head = document::text(
        &databases["binding"]["source"]["snapshot"]["head"],
        "库存构建源码",
    )?;
    for phase in ["before", "after"] {
        let current = document::read(&directory.join(format!("inventory-{phase}.json")))?;
        schema::origins(backend, directory, selected, phase, &current)?;
        logical_inventory::validate_selected(
            &current,
            selected,
            models,
            head,
            document::text(&current["observed_at"], "库存观察时间")?,
            true,
        )?;
        compare_raw(&current, databases, &value["image"]["objects"])?;
        let logical = logical_inventory::logical(&current)?;
        if previous.as_ref().is_some_and(|before| *before != logical) {
            return Err("两次观察库存不同，不能证明只读稳定".into());
        }
        previous = Some(logical);
    }
    Ok(())
}

fn compare_raw(current: &Value, raw: &Value, objects: &Value) -> Result<()> {
    for db in document::array(&current["databases"], "逻辑库存数据库")? {
        let key = document::text(&db["key"], "逻辑库存目标")?;
        let target = &raw["databases"][key]["target"];
        let rows = document::array(&target["database"]["tables"], "业务表")?
            .iter()
            .chain(document::array(&target["preserved_tables"], "保留表")?)
            .map(|row| Ok((document::text(&row["table"], "表名")?, row)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        if document::array(&db["tables"], "逻辑表")?.iter().any(|row| {
            row["table"]
                .as_str()
                .and_then(|name| rows.get(name))
                .is_none_or(|raw| **raw != *row)
        }) || db["placements"] != target["database"]["placements"]
        {
            return Err("观察库存业务表或 placement 与完整原始像不同".into());
        }
    }
    let mut observed = BTreeMap::new();
    for bucket in document::array(&current["objects"], "逻辑对象桶")? {
        let name = document::text(&bucket["bucket"], "逻辑桶名")?;
        let rows = document::array(&bucket["entries"], "逻辑对象")?
            .iter()
            .map(|row| {
                Ok((
                    document::text(&row["key"], "逻辑对象键")?,
                    json!({"bytes":row["bytes"],"sha256":row["sha256"]}),
                ))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        observed.insert(name, rows);
    }
    let expected = document::object(objects, "完整对象像")?
        .iter()
        .map(|(bucket, rows)| {
            let rows = document::object(rows, "完整桶像")?
                .iter()
                .map(|(key, row)| {
                    (
                        key.as_str(),
                        json!({"bytes":row["bytes"],"sha256":row["sha256"]}),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            Ok((bucket.as_str(), rows))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    if observed != expected {
        return Err("观察库存完整对象范围或字节摘要与采集像不同".into());
    }
    Ok(())
}

pub(super) fn scale(image: &Value) -> Result<Value> {
    let mut tenants = std::collections::BTreeSet::new();
    let (mut posts, mut objects, mut bytes) = (0_u64, 0_u64, 0_u64);
    for database in document::object(&image["databases"], "完整四库像")?.values() {
        for placement in document::array(
            &database["target"]["database"]["placements"],
            "完整 placement",
        )? {
            tenants.insert(document::text(&placement["tenant_id"], "placement 租户")?);
        }
        for table in document::array(&database["target"]["database"]["tables"], "完整业务表")?
        {
            if table["table"] == "sys_post" {
                posts = posts
                    .checked_add(table["rows"].as_u64().ok_or("岗位行数无效")?)
                    .ok_or("岗位行数溢出")?;
            }
        }
    }
    for bucket in document::object(&image["objects"], "完整五桶")?.values() {
        let rows = document::object(bucket, "完整对象桶")?;
        objects = objects
            .checked_add(u64::try_from(rows.len())?)
            .ok_or("对象数溢出")?;
        for row in rows.values() {
            bytes = bytes
                .checked_add(row["bytes"].as_u64().ok_or("对象长度无效")?)
                .ok_or("对象总长度溢出")?;
        }
    }
    Ok(json!({"tenants":tenants.len(),"post_rows":posts,"objects":objects,"object_bytes":bytes}))
}
