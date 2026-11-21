//! 从八份原始输出重算四库完整像；不把保存证据提升为当前资源或停机资格。
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::{
        digest, json as document,
        resources::common,
        target_verify::{database::model, guard::Inputs},
    },
};

use super::super::binding;

const KEYS: [&str; 4] = ["shared-control", "shared", "dedicated-a", "dedicated-b"];
const FIELDS: &[&str] = &[
    "format_version",
    "status",
    "side",
    "scope_id",
    "binding_sha256",
    "keys",
    "inventories",
    "observations",
    "remote_writes",
    "producer_stopped_proven",
    "fresh_target_proven",
    "target_ready",
    "clone_verified",
    "restore_qualified",
];

pub(in crate::recovery::source::evidence) fn request(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &["backend", "descriptor", "selected", "execution"],
        "四目标库存复核参数",
    )?;
    let selected = &args["selected"];
    document::exact(selected, &["scope_id", "databases"], "四目标库存复核目标")?;
    let backend = binding::path(&args["backend"])?;
    let execution = binding::path(&args["execution"])?;
    for root in [&backend, &execution] {
        validate_absolute_path(root, LocalTestPathKind::ExistingDirectory)?;
    }
    verify(&backend, &args["descriptor"], selected, &execution)
}

pub(in crate::recovery::source::evidence) fn verify(
    backend: &Path,
    descriptor: &Value,
    selected: &Value,
    execution: &Path,
) -> Result<Value> {
    let path = common::bound_file(backend, descriptor, "四目标完整库存")?;
    let directory = path.parent().ok_or("完整库存缺少目录")?;
    if path.file_name().is_none_or(|name| name != "inventory.json")
        || directory.join("failure.json").try_exists()?
    {
        return Err("四目标完整像路径不正确或存在失败现场".into());
    }
    let names = ["before", "after"]
        .into_iter()
        .flat_map(|phase| KEYS.map(|key| format!("{phase}-target-{key}.json")))
        .collect::<Vec<_>>();
    let mut files = names
        .iter()
        .map(|name| directory.join(name))
        .collect::<Vec<_>>();
    files.extend([
        path.clone(),
        directory.join("binding-before.json"),
        directory.join("binding-after.json"),
    ]);
    let held = Inputs::open(files)?;
    let value = document::read(&path)?;
    header(&value, selected)?;
    let before = document::read(&directory.join("binding-before.json"))?;
    if document::read(&directory.join("binding-after.json"))? != before
        || value["binding_sha256"] != digest::plan_hash(&before)?
    {
        return Err("四目标完整像来源前后绑定不同".into());
    }
    let expected = names.iter().map(String::as_str).collect::<Vec<_>>();
    document::exact(&value["inventories"], &expected, "完整库存八份原始输出")?;
    document::exact(&value["observations"], &KEYS, "完整库存四目标观察")?;
    let (observed, raw) = observations(backend, directory, &value, selected, execution)?;
    if json!(observed) != value["observations"] {
        return Err("四目标逐表、保留表、placement 或 ownership 与原始输出不同".into());
    }
    held.check()?;
    common::bound_file(backend, descriptor, "四目标完整库存")?;
    Ok(json!({"receipt":value,"binding":before,"databases":raw}))
}

fn header(value: &Value, selected: &Value) -> Result<()> {
    document::exact(value, FIELDS, "四目标完整像收据")?;
    if value["format_version"].as_u64() != Some(1)
        || value["status"] != "side_inventory_captured"
        || !matches!(value["side"].as_str(), Some("source" | "target"))
        || value["scope_id"] != selected["scope_id"]
        || value["keys"] != json!(KEYS)
        || value["remote_writes"].as_u64() != Some(0)
        || [
            "producer_stopped_proven",
            "fresh_target_proven",
            "target_ready",
            "clone_verified",
            "restore_qualified",
        ]
        .iter()
        .any(|field| value[*field] != false)
    {
        return Err("四目标完整像收据类型、scope 或资格字段不同".into());
    }
    Ok(())
}

type Observations = (BTreeMap<String, Value>, BTreeMap<String, Value>);

fn observations(
    backend: &Path,
    directory: &Path,
    receipt: &Value,
    selected: &Value,
    execution: &Path,
) -> Result<Observations> {
    let scope = document::text(&selected["scope_id"], "完整库存 scope")?;
    let mut observed = BTreeMap::new();
    let mut raw = BTreeMap::new();
    let mut physical = BTreeSet::new();
    for database in document::array(&selected["databases"], "完整库存四个目标")? {
        let key = document::text(&database["key"], "完整库存目标")?;
        let name = document::text(&database["database"], "完整库存物理库")?;
        if !KEYS.contains(&key) || raw.contains_key(key) || !physical.insert(name) {
            return Err("完整库存四个目标缺失、重复或指向相同物理库".into());
        }
        let first = read_raw(backend, directory, receipt, "before", key)?;
        let second = read_raw(backend, directory, receipt, "after", key)?;
        if first != second {
            return Err("完整数据库像采集期间变化".into());
        }
        let owners = model::expected_owners(scope, database["kind"] == "combined");
        observed.insert(
            key.to_owned(),
            model::observation(execution, &first, database, scope, &owners)?,
        );
        raw.insert(key.to_owned(), first);
    }
    if raw.len() != KEYS.len() {
        return Err("完整库存必须覆盖四个逻辑目标，包括空目标".into());
    }
    Ok((observed, raw))
}

fn read_raw(
    backend: &Path,
    directory: &Path,
    receipt: &Value,
    phase: &str,
    key: &str,
) -> Result<Value> {
    let name = format!("{phase}-target-{key}.json");
    let path = common::common_file(backend, &directory.join(&name), "完整库存原始输出")?;
    let actual = digest::file(&path)?;
    if receipt["inventories"][&name] != json!({"bytes":actual.bytes,"sha256":actual.sha256}) {
        return Err("完整数据库像原始输出摘要变化".into());
    }
    let value = document::read(&path)?;
    if digest::file(&path)? != actual {
        return Err("完整数据库像原始输出读取期间变化".into());
    }
    Ok(value)
}
