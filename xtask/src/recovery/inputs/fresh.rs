use std::{collections::HashSet, path::Path};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    document::{array, exact, integer, object, text},
    domain::BUCKETS,
};
use crate::{Result, recovery::clone::native::target_verify::evidence_protocol::inventory_history};

pub(crate) struct Fresh {
    pub(crate) binding: Value,
    pub(crate) inventory: Value,
    pub(crate) ownership_sha256: String,
}

pub(crate) fn validate(
    backend: &Path,
    path: &Path,
    result: &serde_json::Map<String, Value>,
    initialized: &Value,
    target_request: &Value,
) -> Result<Fresh> {
    let document = super::document::read(path, backend, "fresh target 核验")?;
    if path.file_name().and_then(|name| name.to_str()) != Some("verify.json") {
        return Err("fresh-target verify 文件名无效".into());
    }
    let fresh = exact(
        &document.value,
        &[
            "status",
            "initialized",
            "inventory",
            "remote_writes",
            "target_ready",
            "clone_verified",
            "restore_qualified",
        ],
        "fresh target 核验",
    )?;
    let inventory = exact(
        &fresh["inventory"],
        &["receipt", "observations"],
        "fresh target 库存绑定",
    )?;
    if fresh.get("status").and_then(Value::as_str) != Some("fresh_target_reverified")
        || fresh.get("initialized") != result.get("initialized")
        || integer(fresh.get("remote_writes").unwrap(), "fresh target 远端写入")? != 0
        || ["target_ready", "clone_verified", "restore_qualified"]
            .iter()
            .any(|field| fresh.get(*field) != Some(&Value::Bool(false)))
        || inventory.get("observations")
            != initialized
                .get("inventory")
                .and_then(|value| value.get("observations"))
    {
        return Err("fresh-target verify 未绑定同一初始化目标及完整前像".into());
    }
    let receipt_binding = inventory["receipt"].clone();
    let (_, receipt_document) =
        super::document::bound_file(backend, &receipt_binding, "fresh target 库存收据")?;
    let receipt = exact(
        &receipt_document.value,
        &[
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
        ],
        "fresh target 库存收据",
    )?;
    if super::document::digest(
        receipt.get("binding_sha256").ok_or("库存绑定摘要缺失")?,
        "fresh target 库存绑定摘要",
    )
    .is_err()
        || receipt.get("remote_writes") != Some(&Value::from(0))
        || [
            "producer_stopped_proven",
            "fresh_target_proven",
            "target_ready",
            "clone_verified",
            "restore_qualified",
        ]
        .iter()
        .any(|field| receipt.get(*field) != Some(&Value::Bool(false)))
    {
        return Err("fresh-target 库存收据必须保持只读且不能冒充恢复证明".into());
    }
    inventory_history::verify_target_observations(
        &receipt_document.value,
        target_request,
        &inventory["observations"],
    )?;
    let ownership = ownership_sha256(initialized, target_request)?;
    document.assert_unchanged()?;
    receipt_document.assert_unchanged()?;
    Ok(Fresh {
        binding: super::document::binding(&document),
        inventory: receipt_binding,
        ownership_sha256: ownership,
    })
}

fn ownership_sha256(initialized: &Value, target_request: &Value) -> Result<String> {
    let initialized = object(initialized, "fresh target initialized")?;
    let inventory = object(
        initialized.get("inventory").ok_or("初始化缺少库存")?,
        "初始化库存",
    )?;
    let observations = object(
        inventory.get("observations").ok_or("初始化缺少观察")?,
        "初始化观察",
    )?;
    let objects = object(
        initialized.get("objects").ok_or("初始化缺少对象")?,
        "初始化对象",
    )?;
    let names = objects.keys().map(String::as_str).collect::<HashSet<_>>();
    if names != BUCKETS.into_iter().collect() {
        return Err("fresh target ownership 未覆盖全部对象桶".into());
    }
    let target_databases = array(
        target_request
            .get("target")
            .and_then(|value| value.get("databases"))
            .ok_or("新目标请求缺少数据库")?,
        "新目标请求数据库",
    )?;
    let target_database_names = target_databases
        .iter()
        .map(|database| text(database.get("database").unwrap(), "新目标数据库名"))
        .collect::<Result<HashSet<_>>>()?;
    if observations
        .keys()
        .map(String::as_str)
        .collect::<HashSet<_>>()
        != target_database_names
    {
        return Err("fresh target ownership 未覆盖全部数据库".into());
    }
    let scope = text(
        target_request
            .get("target")
            .and_then(|value| value.get("scope_id"))
            .ok_or("新目标请求缺少 scope")?,
        "新目标 scope",
    )?;
    let mut owners = serde_json::Map::new();
    for database in target_databases {
        let name = text(database.get("database").unwrap(), "新目标数据库名")?;
        let key = text(database.get("key").unwrap(), "新目标数据库键")?;
        let observation = observations
            .get(name)
            .ok_or("fresh target ownership 未覆盖全部数据库")?;
        let observation = object(observation, "数据库 ownership")?;
        let kind = text(database.get("kind").unwrap(), "新目标数据库类型")?;
        let kinds: &[&str] = if kind == "combined" {
            &["control", "tenant-data"]
        } else {
            &["tenant-data"]
        };
        let expected = kinds
            .iter()
            .map(|kind| {
                json!({
                    "resource_kind": kind,
                    "scope_id": scope,
                    "marker": format!("ryframe-owner:v1:{scope}:{kind}"),
                })
            })
            .collect::<Vec<_>>();
        if observation.get("ownership") != Some(&Value::Array(expected)) {
            return Err("fresh target 数据库 ownership 不属于精确目标".into());
        }
        owners.insert(
            key.to_owned(),
            observation.get("ownership").unwrap().clone(),
        );
    }
    for bucket in BUCKETS {
        let marker = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
        let expected = json!({
            "keys": [format!("{scope}/.ryframe-owner")],
            "owner": {
                "bytes": marker.len(),
                "sha256": hex::encode(Sha256::digest(marker.as_bytes())),
            },
        });
        if objects.get(bucket) != Some(&expected) {
            return Err("fresh target 对象 ownership 与唯一空前缀不符".into());
        }
    }
    let redis = initialized
        .get("redis")
        .ok_or("初始化缺少 Redis ownership")?;
    super::document::hash(&json!({
        "databases": owners,
        "objects": objects,
        "redis": redis,
    }))
}
