//! 已发布 seed 身份的离线核验；不取得运行锁、不创建或补签收据。
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path};

use super::{evidence, plan, receipt, state};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as document, resources::common},
};

pub(crate) fn projection(
    backend: &Path,
    plan: &Value,
    root: &Path,
    allow_absent: bool,
) -> Result<Value> {
    validate_local_test_path(root, backend, LocalTestPathKind::StateDirectory)?;
    if !root.exists() {
        if !allow_absent {
            return Err("身份账本缺失或路径不是明确未创建状态".into());
        }
        return Ok(
            json!({"state":"absent","identity_state":root,"plan_sha256":plan["plan_sha256"],
            "ledger":null,"projection_sha256":null,"entries":0,"roles":0,"users":0}),
        );
    }
    validate_local_test_path(root, backend, LocalTestPathKind::ExistingDirectory)?;
    match fs::symlink_metadata(root.join("lock")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err("身份账本仍持锁或锁状态未知".into()),
    }
    let filename = root.join("ledger.json");
    let before = common::binding(&filename)?;
    let ledger = document::read(&filename)?;
    state::validate(plan, &ledger, true)?;
    if !matches!(
        ledger["status"].as_str(),
        Some("prepared" | "verifying" | "verified")
    ) || ledger["entries"].as_array().map(Vec::len) != Some(622)
        || ledger["roles"].as_object().map(serde_json::Map::len) != Some(11)
        || ledger["users"].as_object().map(serde_json::Map::len) != Some(200)
    {
        return Err("身份账本不是全部确认的固定准备结果".into());
    }
    let stable = json!({"format_version":ledger["format_version"],"plan_sha256":ledger["plan_sha256"],
        "entries":ledger["entries"],"roles":ledger["roles"],"users":ledger["users"]});
    if common::binding(&filename)? != before {
        return Err("身份账本在投影读取期间变化".into());
    }
    Ok(
        json!({"state":ledger["status"],"identity_state":root,"plan_sha256":plan["plan_sha256"],
        "ledger":before,"projection_sha256":digest::plan_hash(&stable)?,
        "entries":622,"roles":11,"users":200}),
    )
}

pub(crate) fn names(plan_sha256: &str, root: &Path) -> (String, String) {
    let value = evidence::hash(format!("{plan_sha256}\0{}", root.display()));
    let mut chars = value.as_bytes()[..32].to_vec();
    chars[12] = b'5';
    chars[16] = b'8';
    let raw = String::from_utf8(chars).expect("SHA-256 仅包含 ASCII");
    let id = format!(
        "{}-{}-{}-{}-{}",
        &raw[..8],
        &raw[8..12],
        &raw[12..16],
        &raw[16..20],
        &raw[20..]
    );
    (
        format!("identity-publication-{id}.json"),
        format!("verified-{id}.json"),
    )
}

pub(crate) fn verify(backend: &Path, prepared: &Value, root: &Path) -> Result<Value> {
    plan::load(prepared, backend)?;
    let projected = projection(backend, prepared, root, false)?;
    if projected["state"] != "verified" {
        return Err("身份账本不是严格有序的完整 verified 投影".into());
    }
    let ledger_binding = projected["ledger"].clone();
    let ledger = common::read_bound(backend, &ledger_binding, "seed 身份账本")?;
    let (intent_name, receipt_name) = names(
        prepared["plan_sha256"].as_str().ok_or("身份计划摘要缺失")?,
        root,
    );
    let expected = BTreeSet::from([intent_name.clone(), receipt_name.clone()]);
    if publications(root)? != expected || ledger["verified_receipt"] != receipt_name {
        return Err("身份发布必须只有确定性 intent 和 verified 收据且不能残留 pending".into());
    }
    let intent_path = root.join(&intent_name);
    let receipt_path = root.join(&receipt_name);
    let intent_binding = common::binding(&intent_path)?;
    let receipt_binding = common::binding(&receipt_path)?;
    let intent = document::read(&intent_path)?;
    let verified = document::read(&receipt_path)?;
    document::exact(
        &intent,
        &[
            "format_version",
            "kind",
            "plan_sha256",
            "identity_state",
            "publication_nonce",
            "ledger_sha256",
            "receipt_file",
            "receipt_sha256",
            "receipt",
        ],
        "身份发布 intent",
    )?;
    let mut initial = ledger.clone();
    initial["status"] = json!("prepared");
    for name in ["verified_receipt", "publication_nonce"] {
        initial
            .as_object_mut()
            .ok_or("身份账本必须为对象")?
            .remove(name);
    }
    if intent["format_version"].as_u64() != Some(1)
        || intent["kind"] != "devex-identity-verification-publication"
        || intent["plan_sha256"] != prepared["plan_sha256"]
        || intent["identity_state"].as_str() != root.to_str()
        || intent["publication_nonce"] != ledger["publication_nonce"]
        || intent["ledger_sha256"].as_str() != Some(evidence::encoded_hash(&initial)?.as_str())
        || intent["receipt_file"] != receipt_name
        || intent["receipt_sha256"] != receipt_binding["sha256"]
        || intent["receipt"] != verified
    {
        return Err("身份 publication intent 未完整绑定计划、根目录、账本和收据".into());
    }
    receipt::validate(&verified, prepared, &ledger, root)?;
    let templates = plan::array(&verified["templates"])?
        .iter()
        .map(|item| common::binding(Path::new(plan::text(item, "path")?)))
        .collect::<Result<Vec<_>>>()?;
    if common::binding(&root.join("ledger.json"))? != ledger_binding
        || common::binding(&intent_path)? != intent_binding
        || common::binding(&receipt_path)? != receipt_binding
        || publications(root)? != expected
        || projection(backend, prepared, root, false)? != projected
    {
        return Err("身份账本、publication 或模板在读取期间变化".into());
    }
    receipt::validate(&verified, prepared, &ledger, root)?;
    Ok(
        json!({"ledger":ledger_binding,"publication_intent":intent_binding,"verified":receipt_binding,"templates":templates}),
    )
}

fn publications(root: &Path) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    for entry in fs::read_dir(root)? {
        let name = entry?
            .file_name()
            .into_string()
            .map_err(|_| "身份发布文件名不是 UTF-8")?;
        let lower = name.to_lowercase();
        if lower.starts_with("identity-publication-") || lower.starts_with("verified-") {
            found.insert(name);
        }
    }
    Ok(found)
}
