//! 重读完整初始化历史；持锁模式只接受保护同一目标的真实 Generation。
use std::{fs, io::ErrorKind, path::Path};

use serde_json::{Value, json};

use super::{
    super::digest, binding_request, evidence_model::*, evidence_protocol::inventory_history, guard,
    proof,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

mod failure;
pub(super) mod tree;

pub(crate) fn request_binding(backend: &Path, request: &Value) -> Result<Value> {
    binding_request::validate(backend, request, &Value::Null)
}

fn unlocked(target: &Path) -> Result<()> {
    match fs::symlink_metadata(target.join("initialize.lock")) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("初始化历史仍有 initialize.lock；本入口不支持持锁 checkpoint".into()),
    }
}

pub(crate) fn unlocked_history(backend: &Path, filename: &Path) -> Result<(Value, Value)> {
    history(backend, filename, None)
}

pub(crate) fn owned_history(
    backend: &Path,
    filename: &Path,
    owner: &guard::Generation,
) -> Result<(Value, Value)> {
    history(backend, filename, Some(owner))
}

/// 已发布来源只消费原阶段事实；当前树用于防漂移，不补称历史全树封存。
pub(crate) fn evidence_history(
    backend: &Path,
    filename: &Path,
    owner: Option<&guard::Generation>,
) -> Result<(Value, Value)> {
    let target = filename.parent().ok_or("初始化历史缺少目标目录")?;
    validate_local_test_path(target, backend, LocalTestPathKind::ExistingDirectory)?;
    check(
        filename == target.join("initialized.json"),
        "初始化历史路径无效",
    )?;
    protection(target, owner)?;
    let before = super::tree::snapshot(backend, target, owner.is_some())?;
    let inputs = guard::Inputs::open(super::tree::files(target, &before)?)?;
    let resolved = failure::resolved(backend, target, owner)?;
    let initialized = read(backend, filename)?;
    let prepared = read(backend, &target.join("prepare.json"))?;
    let request = read(backend, &bound(backend, &prepared["request"])?)?;
    initialization(backend, target, &initialized, &prepared, &request, resolved)?;
    if super::tree::snapshot(backend, target, owner.is_some())? != before {
        return Err("已发布 seed 初始化内容或当前文件树在核验期间变化".into());
    }
    inputs.check()?;
    protection(target, owner)?;
    Ok((initialized, request))
}

pub(super) fn unresolved(backend: &Path, target: &Path) -> bool {
    failure::resolved(backend, target, None).is_err()
}

fn protection(target: &Path, owner: Option<&guard::Generation>) -> Result<()> {
    match owner {
        Some(owner) => owner.protects(target),
        None => unlocked(target),
    }
}

fn history(
    backend: &Path,
    filename: &Path,
    owner: Option<&guard::Generation>,
) -> Result<(Value, Value)> {
    let target = filename.parent().ok_or("初始化历史缺少目标目录")?;
    validate_local_test_path(target, backend, LocalTestPathKind::ExistingDirectory)?;
    check(
        filename == target.join("initialized.json"),
        "初始化历史必须读取固定 initialized.json",
    )?;
    protection(target, owner)?;
    let locked = owner.is_some();
    let before = tree::initialized(backend, target, locked)?;
    let files = super::tree::snapshot(backend, target, locked)?;
    let inputs = guard::Inputs::open(super::tree::files(target, &files)?)?;
    let resumed = failure::resolved(backend, target, owner)?;
    let initialized = read(backend, filename)?;
    let prepared = read(backend, &target.join("prepare.json"))?;
    let request = read(backend, &bound(backend, &prepared["request"])?)?;
    request_binding(backend, &request)?;
    initialization(backend, target, &initialized, &prepared, &request, resumed)?;
    inputs.check()?;
    protection(target, owner)?;
    check(
        tree::initialized(backend, target, locked)? == before
            && super::tree::snapshot(backend, target, locked)? == files,
        "初始化历史读取期间文件树或绑定变化",
    )?;
    protection(target, owner)?;
    Ok((initialized, request))
}

fn initialization(
    backend: &Path,
    target: &Path,
    initialized: &Value,
    prepared: &Value,
    request: &Value,
    resumed: bool,
) -> Result<()> {
    check(
        initialized["status"] == "fresh_target_initialized"
            && initialized["controlled_generation_never_started"] == true
            && initialized["prepare_sha256"]
                == binding(backend, &target.join("prepare.json"))?["sha256"]
            && read(backend, &target.join("initialized-candidate.json"))? == *initialized
            && read(backend, &target.join("request.json"))? == *request
            && initialized["history"] == proof::history(backend, target)?
            && initialized["generation"] == prepared["generation"]
            && prepared["request_sha256"] == digest::plan_hash(request)?
            && initialized["id"] == request["id"]
            && initialized["scope_id"] == request["target"]["scope_id"],
        "初始化历史候选、请求、代次或未启动声明不一致",
    )?;
    inventory_history::verify(
        backend,
        target,
        &json!({
            "initial":initialized["inventory"], "request":request,
            "resumed":resumed,"inventory_directory":null
        }),
    )?;
    check(
        super::evidence_reset::history(backend, target, request, &initialized["generation"])?["completed"]
            == initialized["reset"],
        "初始化历史 reset 完成证据变化",
    )
}
