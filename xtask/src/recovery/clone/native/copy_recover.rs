//! 回收已退出的复制控制器所持的两把原锁，保留账本及远端状态。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    copy_owner::{Commands, Owner, Role},
    json as document,
    lifecycle::{
        lock::{ProcessGuard, RunLock},
        manifest, state,
    },
    resources::common,
    target_verify::{guard::Inputs, publication},
};
use crate::Result;

const ROLES: [Role; 2] = [Role::Target, Role::Ledger];

pub(super) fn run(backend: &Path, directory: &Path, descriptor: &Path) -> Result<Value> {
    let directory = common::local_directory(backend, directory, "复制运行目录")?;
    let descriptor = common::common_file(backend, descriptor, "复制锁归属描述符")?;
    let guard = ProcessGuard::acquire(&directory)?;
    let run_lock = RunLock::claim(&directory)?;
    let result = recover_owned(backend, &directory, &descriptor);
    let release = combine(run_lock.release(), guard.check());
    combine(result, release)
}

fn recover_owned(backend: &Path, directory: &Path, descriptor: &Path) -> Result<Value> {
    let manifest_path = directory.join("manifest.json");
    let inputs = Inputs::open([
        manifest_path.clone(),
        directory.join("state.json"),
        descriptor.to_path_buf(),
    ])?;
    let manifest = manifest::load(backend, &manifest_path)?;
    let state = state::load(directory, false)?;
    if state::running_attempt(&state)?.is_some() {
        return Err("请先核对并收尾原运行控制器，保留正在执行的复制阶段".into());
    }
    let (export, result_path) = source_export(backend, &manifest.value, &state)?;
    let result_input = Inputs::open(result_path)?;
    if source_export(backend, &manifest.value, &state)?.0 != export {
        return Err("原导出阶段结果在绑定期间发生变化".into());
    }
    let copy = common::local_directory(
        backend,
        Path::new(document::text(
            &manifest.value["copy_directory"],
            "复制目录",
        )?),
        "复制目录",
    )?;
    let owner_binding = document::read(descriptor)?;
    let owner = Owner::read(
        backend,
        &copy,
        &owner_binding,
        &export,
        &manifest.value["initialized"],
    )?;
    let checkpoint = || {
        inputs.check()?;
        result_input.check()
    };
    let result = reclaim(&copy, &owner, &owner_binding, &checkpoint);
    combine(result, checkpoint())
}

fn source_export(
    backend: &Path,
    manifest: &Value,
    state: &Value,
) -> Result<(Value, Option<PathBuf>)> {
    if !manifest["source_export"].is_null() {
        return Ok((manifest["source_export"].clone(), None));
    }
    let attempt = state::attempts(state)?
        .iter()
        .rev()
        .find(|item| item["stage"] == "export" && item["status"] == "passed")
        .ok_or("锁回收必须绑定已完成的原导出阶段")?;
    let result = common::bound_file(backend, &attempt["result"], "原导出阶段结果")?;
    let value = document::read(&result)?;
    common::bound_file(backend, &value["export"], "原导出收据")?;
    Ok((value["export"].clone(), Some(result)))
}

struct Reclaim<'a> {
    owner: &'a Owner,
    guards: Vec<(Role, ProcessGuard)>,
    commands: Commands<'a>,
    checkpoint: &'a dyn Fn() -> Result<()>,
}

impl Reclaim<'_> {
    fn check(&self) -> Result<Option<Value>> {
        (self.checkpoint)()?;
        for (_, guard) in &self.guards {
            guard.check()?;
        }
        self.commands.check()?;
        self.owner.observe_stopped()
    }

    fn guard(&self, role: Role) -> Result<&ProcessGuard> {
        self.guards
            .iter()
            .find_map(|(candidate, guard)| (*candidate == role).then_some(guard))
            .ok_or_else(|| "未取得对应复制锁 guard，保留现场".into())
    }
}

pub(super) fn reclaim(
    directory: &Path,
    owner: &Owner,
    owner_binding: &Value,
    checkpoint: &dyn Fn() -> Result<()>,
) -> Result<Value> {
    let guards = acquire_guards(owner)?;
    let window = Reclaim {
        owner,
        guards,
        commands: owner.verify_commands()?,
        checkpoint,
    };
    let observed = window.check()?;
    let mut locks = Vec::new();
    for role in ROLES {
        if let Some(record) = owner.verify_lock(role)? {
            window.guard(role)?;
            locks.push((role, record));
        }
    }
    window.check()?;
    let mut result = json!({
        "format_version": 1, "kind": "devex-copy-lock-recovery", "owner_receipt": owner_binding,
        "observed_controller": observed, "locks": locks.iter().map(|(_, lock)| lock).collect::<Vec<_>>(),
        "remote_writes": 0, "remote_writes_reconciled": false, "target_ready": false,
        "descendant_scope": "仅检查已登记直接命令的退出记录；不证明未登记后代已退出",
        "requires_reconcile": true, "status": "prepared",
    });
    let path = directory.join(format!(
        "copy-lock-recovery-{}.json",
        uuid::Uuid::new_v4().simple()
    ));
    publication::publish(&path, &result)?;
    let prepared = common::binding(&path)?;
    for (role, record) in &locks {
        window.check()?;
        owner.release_recovered_lock(*role, window.guard(*role)?, record)?;
    }
    window.check()?;
    for role in ROLES {
        if owner.verify_lock(role)?.is_some() {
            return Err("复制锁回收后仍存在锁，保留准备收据".into());
        }
    }
    if common::binding(&path)? != prepared || document::read(&path)? != result {
        return Err("复制锁回收准备收据已变化，拒绝发布成功".into());
    }
    result["status"] = json!("owned_locks_released");
    document::write_replace(&path, &result)?;
    window.check()?;
    if document::read(&path)? != result {
        return Err("复制锁回收成功收据发布后发生变化".into());
    }
    result["receipt"] = common::binding(&path)?;
    Ok(result)
}

fn acquire_guards(owner: &Owner) -> Result<Vec<(Role, ProcessGuard)>> {
    let mut guards = Vec::new();
    for role in ROLES {
        let parent = owner.lock_path(role).parent().ok_or("复制锁缺少父目录")?;
        match fs::symlink_metadata(parent) {
            Ok(_) => guards.push((
                role,
                ProcessGuard::acquire_named(parent, role.guard_name())?,
            )),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && matches!(role, Role::Ledger) => {
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(guards)
}

fn combine<T>(primary: Result<T>, checked: Result<()>) -> Result<T> {
    match (primary, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(check)) => Err(format!("{error}；复制锁收尾核对失败：{check}").into()),
    }
}
