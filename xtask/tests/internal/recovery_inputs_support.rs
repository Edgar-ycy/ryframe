use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Object = serde_json::Map<String, Value>;

pub(crate) fn observations(selected: &Value) -> Value {
    let scope = selected["scope_id"].as_str().unwrap();
    let mut result = Object::new();
    for database in selected["databases"].as_array().unwrap() {
        let combined = database["key"].as_str().unwrap() == "shared-control";
        let ownership = if combined {
            json!([
                {"resource_kind": "control", "scope_id": scope, "marker": format!("ryframe-owner:v1:{scope}:control")},
                {"resource_kind": "tenant-data", "scope_id": scope, "marker": format!("ryframe-owner:v1:{scope}:tenant-data")},
            ])
        } else {
            json!([
                {"resource_kind": "tenant-data", "scope_id": scope, "marker": format!("ryframe-owner:v1:{scope}:tenant-data")},
            ])
        };
        result.insert(
            database["database"].as_str().unwrap().into(),
            json!({
                "resource":{
                    "kind":"database",
                    "scope_id":scope,
                    "server_uuid":database["expected_server_uuid"],
                    "database":database["database"],
                },
                "ownership":ownership,
            }),
        );
    }
    Value::Object(result)
}

pub(crate) fn object_ownership(selected: &Value) -> Value {
    let scope = selected["scope_id"].as_str().unwrap();
    let mut result = Object::new();
    for bucket in ["uploads", "avatar", "exports", "imports", "config-packages"] {
        let marker = format!("ryframe-owner:v1:{scope}:object-storage:{bucket}");
        result.insert(
            bucket.into(),
            json!({
                "keys": [format!("{scope}/.ryframe-owner")],
                "owner": {
                    "bytes": marker.len(),
                    "sha256": hex::encode(Sha256::digest(marker.as_bytes())),
                },
            }),
        );
    }
    Value::Object(result)
}

pub(crate) fn review_binding(path: &Path, review: &Value) -> Value {
    let mut value = binding(path);
    value
        .as_object_mut()
        .unwrap()
        .insert("canonical_sha256".into(), Value::String(hash(review)));
    value
}

pub(crate) fn write_value(root: &Path, name: &str, value: &Value) -> PathBuf {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    path
}

pub(crate) fn read_value(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

pub(crate) fn binding(path: &Path) -> Value {
    json!({
        "path": path.to_string_lossy(),
        "bytes": fs::metadata(path).unwrap().len(),
        "sha256": file_sha(path),
    })
}

pub(crate) fn file_sha(path: &Path) -> String {
    hex::encode(Sha256::digest(fs::read(path).unwrap()))
}

pub(crate) fn hash(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub(crate) fn now_seconds() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(crate) fn capture_target_plan(
    root: &Path,
    reference: &Path,
    backup: &Path,
    comparison: &Path,
    arm: &Path,
    fresh: &Path,
    product: &Path,
) -> super::Result<Value> {
    let reference = super::recovery::inputs::document::read(reference, root, "参考计划")?;
    let side = reference
        .value
        .get("target_side")
        .and_then(Value::as_str)
        .ok_or("参考计划缺少目标侧")?
        .to_owned();
    let chain = super::recovery::inputs::chain::load_with_reference(
        root, reference, backup, comparison, arm, fresh, &side,
    )?;
    let product = super::recovery::inputs::document::read(product, root, "产品恢复计划")?;
    super::recovery::inputs::target_plan::capture(root, &chain, &product)
}

// 与正式运行收据共用配置身份算法，测试只创建明确绑定的私有文件。
pub(crate) fn object_configuration(directory: &Path, side: &str) -> String {
    fs::create_dir_all(directory).unwrap();
    fs::write(directory.join("app.test.toml"), format!(
        "[object_storage]\nendpoint = 'http://127.0.0.1:19000'\nregion = 'us-east-1'\naccess_key = '{side}-access'\nsecret_key = '{side}-secret'\n"
    )).unwrap();
    let environment = std::collections::BTreeMap::from([("APP_ENV".into(), "test".into())]);
    let root = super::workspace::root_dir();
    let configuration = &super::recovery::clone::native::target_verify::input::request_configuration::configuration_files;
    let (directory, files) = configuration(&root, &environment, Some(&json!(directory))).unwrap();
    super::recovery::clone::native::target_verify::input::request_configuration::configuration_identity(&directory, &files).unwrap()
}
