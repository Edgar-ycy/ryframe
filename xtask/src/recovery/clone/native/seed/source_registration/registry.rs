//! 将已验证谱系中的全部创建代次按原顺序投影为源生产者登记。
use crate::{Result, recovery::clone::native::json as document};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

type IdentityKey = (u64, String);
type RuntimeRoles<'a> = BTreeMap<IdentityKey, (&'a str, &'a Value)>;

pub(crate) fn build(
    lineage: &Value,
    runtime: &Path,
    runtime_binding: &Value,
    scope_id: &str,
) -> Result<Value> {
    document::object(lineage, "producer lineage")?;
    let binding = document::exact(
        runtime_binding,
        &["path", "bytes", "sha256"],
        "runtime 绑定",
    )?;
    document::text(&binding["path"], "runtime 绑定路径")?;
    let sha = document::text(&binding["sha256"], "runtime 摘要")?;
    if binding["bytes"].as_u64().is_none()
        || sha.len() != 64
        || !sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("runtime 绑定字节数或摘要无效".into());
    }
    let roles = runtime_identities(lineage, runtime)?;
    let identities = document::array(&lineage["identities"], "谱系创建代次")?;
    let mut keys = BTreeSet::new();
    let mut processes = Vec::new();
    let mut sequence = 0_usize;
    let mut names = BTreeSet::new();
    for item in identities {
        document::object(item, "谱系身份记录")?;
        let identity = &item["identity"];
        let key = identity_key(identity)?;
        if !keys.insert(key.clone()) {
            return Err("producer lineage 创建代次重复或不足".into());
        }
        let role = if let Some((role, expected)) = roles.get(&key) {
            if identity != *expected {
                return Err("seed runtime 角色身份不在完整谱系集合".into());
            }
            (*role).to_owned()
        } else {
            sequence += 1;
            format!("history-{sequence:04}")
        };
        if !names.insert(role.clone()) {
            return Err("producer registry 名称重复".into());
        }
        processes.push(json!({"name":role,"identity":identity.clone()}));
    }
    if keys.len() < 3 {
        return Err("producer lineage 创建代次重复或不足".into());
    }
    if !roles.keys().all(|key| keys.contains(key)) {
        return Err("seed runtime 角色身份不在完整谱系集合".into());
    }
    Ok(json!({"format_version":1,"scope_id":scope_id,"runtime_sha256":sha,"processes":processes}))
}

fn runtime_identities<'a>(lineage: &'a Value, runtime: &Path) -> Result<RuntimeRoles<'a>> {
    let runtime = runtime.to_str().ok_or("runtime 路径必须为 UTF-8")?;
    let mut selected = None;
    for item in document::array(&lineage["runtimes"], "runtime 谱系")? {
        document::object(item, "runtime 谱系项")?;
        let directory = document::text(&item["runtime_directory"], "runtime 目录")?;
        if directory == runtime && selected.replace(item).is_some() {
            return Err("producer lineage 未唯一覆盖 seed runtime".into());
        }
    }
    let selected = selected.ok_or("producer lineage 未唯一覆盖 seed runtime")?;
    let mut result = BTreeMap::new();
    let mut roles = BTreeSet::new();
    for receipt in document::array(&selected["role_receipts"], "runtime 角色收据")? {
        document::object(receipt, "runtime 角色收据")?;
        let role = document::text(&receipt["role"], "runtime 角色")?;
        if !matches!(role, "api" | "worker") || !roles.insert(role) {
            return Err("seed runtime 最新 API/Worker 收据不完整".into());
        }
        let identity = &receipt["identity"];
        if result
            .insert(identity_key(identity)?, (role, identity))
            .is_some()
        {
            return Err("seed runtime API/Worker 创建身份重复".into());
        }
    }
    if roles.len() != 2 {
        return Err("seed runtime 最新 API/Worker 收据不完整".into());
    }
    Ok(result)
}

fn identity_key(identity: &Value) -> Result<IdentityKey> {
    document::object(identity, "生产者创建身份")?;
    let pid = identity["pid"]
        .as_u64()
        .filter(|n| *n > 1)
        .ok_or("生产者 PID 无效")?;
    let started = document::text(&identity["started"], "生产者创建代次")?;
    document::text(&identity["executable"], "生产者可执行路径")?;
    Ok((pid, started.to_owned()))
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/registry.rs"]
mod tests;
