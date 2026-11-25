//! 只核对已开始的首个用户创建，不使用原创建命令或密码重置命令。

use std::{fs, io::ErrorKind, path::Path};

use serde_json::{Value, json};

use super::{evidence, inspection, ledger::Ledger, plan, state};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{digest, json as document, resources::common},
};

pub(crate) fn first_create(
    plan: &Value,
    root: &Path,
    expected_before: &str,
    original: &Value,
    audit: &Value,
    check: &mut dyn FnMut() -> Result<()>,
) -> Result<Value> {
    let backend = Path::new(plan::text(&plan["environment"], "backend_dir")?);
    validate_local_test_path(root, backend, LocalTestPathKind::ExistingDirectory)?;
    plan::load(plan, backend)?;
    let output = root.join("first-create-reconciliation");
    let path = output.join("ledger-before.json");
    let before = match fs::symlink_metadata(&path) {
        Ok(_) => bound_before(backend, &path, expected_before)?,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            bound_before(backend, &root.join("ledger.json"), expected_before)?
        }
        Err(error) => return Err(error.into()),
    };
    let group = plan::array(&plan["groups"])?
        .first()
        .ok_or("身份计划缺少首组")?;
    let user = plan::array(&group["users"])?
        .first()
        .ok_or("身份计划缺少首个用户")?;
    let slot = plan::text(user, "user_slot")?;
    let role = state::id(&before["roles"][plan::text(group, "slot")?])?;
    check()?;
    let observation = inspection::observe_created_user(plan, group, user, role, None, Some(audit))?;
    let id = state::id(&observation["id"])?;
    if observation["role_id"] != role || observation["audit"].is_null() {
        return Err("身份创建后像未证明原角色与同请求成功审计".into());
    }
    let authority = json!({
        "original": original,
        "observation": observation,
        "original_http_response_id_proven": false,
    });
    check()?;
    match fs::create_dir(&output) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    Ledger::reconcile_first_create(
        plan,
        root,
        &output,
        expected_before,
        slot,
        id,
        &authority,
        check,
    )
}

pub(crate) fn original_lock_release(
    backend: &Path,
    root: &Path,
    descriptor: &Value,
    process: &Value,
    plan: &Value,
) -> Result<()> {
    let path = common::bound_file(backend, descriptor, "原身份账本锁释放收据")?;
    let release = common::read_bound(backend, descriptor, "原身份账本锁释放收据")?;
    let release_id = plan::text(&release, "release_id")?;
    if !super::run_lock::uuid(release_id)
        || path
            != root
                .join("lock-releases")
                .join(format!("{release_id}.released.json"))
    {
        return Err("原身份账本锁释放收据不属于固定账本".into());
    }
    let lock = json!({
        "format_version": 1,
        "kind": "devex-identity-run-lock",
        "pid": process["identity"]["pid"],
        "plan_sha256": plan["plan_sha256"],
        "owner_token": process["identity_lock_owner"],
    });
    let tombstone = format!("{release_id}.lock");
    let lock_bytes = evidence::encode(&lock)?;
    let expected = json!({
        "format_version": 1,
        "kind": "devex-identity-run-lock-release",
        "release_id": release_id,
        "tombstone_file": tombstone,
        "lock_sha256": evidence::hash(&lock_bytes),
        "lock": lock,
    });
    let file = root.join("lock-releases").join(&tombstone);
    validate_local_test_path(&file, backend, LocalTestPathKind::ExistingFile)?;
    if release != expected || evidence::regular(&file)?.as_deref() != Some(lock_bytes.as_slice()) {
        return Err("原身份锁墓碑、所有者或计划摘要不一致".into());
    }
    Ok(())
}

fn bound_before(backend: &Path, path: &Path, sha: &str) -> Result<Value> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingFile)?;
    let bytes = evidence::regular(path)?.ok_or("原身份账本前像不存在")?;
    if evidence::hash(&bytes) != sha || digest::file(path)?.sha256 != sha {
        return Err("待和解的原身份账本前像与已登记摘要不同".into());
    }
    let value = evidence::parse(&bytes)?;
    document::object(&value, "身份和解账本前像")?;
    Ok(value)
}
