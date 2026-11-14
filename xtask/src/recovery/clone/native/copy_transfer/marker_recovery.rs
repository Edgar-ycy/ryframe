//! 仅恢复已完成初始化、尚未完成复制的原本机目标 Redis 标记。
use std::path::Path;

use serde_json::{Value, json};

use super::super::{
    bridge::scope::Scope,
    cache::handoff::locks::Storage,
    lifecycle::{
        lock::ProcessGuard,
        manifest,
        stage::{StageRequest, StageRunner},
    },
    post_process::files::{absent, combine},
    resources::common,
    target_verify::{
        fresh::initialize::marker,
        guard::Generation,
        redis,
        storage::{StorageObserver, SystemObserver},
    },
};
use super::target_port::Target;
use crate::Result;

mod authority;
mod receipts;
use authority::{Recovery, Stage};
use receipts::Receipts;

pub(crate) struct MarkerRecovery;

impl StageRunner for MarkerRecovery {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        authority::preflight(request, &manifest.value)
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        let manifest = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        authority::preflight(request, &manifest.value)?;
        let stage = Stage::load(request, &manifest.value)?;
        let storage = Storage::acquire(request.backend, request.run_dir, &manifest.value)?;
        let result = with_storage(&stage, &storage);
        storage.finish(result)
    }
}

fn with_storage(stage: &Stage<'_>, storage: &Storage) -> Result<Value> {
    let owner = Generation::acquire(stage.request.backend, &stage.target)?;
    let result = (|| {
        let lock = stage.ledger.join("clone.lock");
        absent(&lock)?;
        let fence = ProcessGuard::acquire_named(&stage.ledger, "copy-ledger.guard")?;
        absent(&lock)?;
        owner.check_initialization_lock()?;
        let copy = stage.ledger.parent().ok_or("原复制账本缺少目录")?;
        let target = Target::bind(
            stage.request.backend,
            stage.manifest,
            &storage.directory,
            copy,
            Some(&owner),
            || Scope::load(stage.request.backend, &[], &[]),
        )?;
        let recovery = Recovery::load(stage, storage, &owner, &fence, &target)?;
        let result = execute(&recovery, &target);
        combine(result, recovery.check())
    })();
    owner.finish(result)
}

fn execute(recovery: &Recovery<'_>, target: &Target) -> Result<Value> {
    let stage = recovery.stage;
    let selected = &target.storage.value["redis"];
    let receipts = Receipts::open(recovery)?;
    let inputs = target.storage_inputs(&receipts.directory);
    let checkpoint = || {
        recovery.check()?;
        receipts.check()?;
        Ok(())
    };
    let observed = |command: &mut dyn FnMut(&[&str]) -> Result<Value>| {
        let image = redis::marker_image(&inputs, command)?;
        if !redis::info_matches(selected, &command(&["INFO", "server"])?)? {
            return Err("Redis 标记观察不能证明原夹具后继连接".into());
        }
        Ok(image)
    };
    if stage.request.mode == "reconcile" {
        return receipts.reconcile(
            |head| recovery.check_prefix(head),
            || SystemObserver.redis_window(&inputs, selected, observed),
            &checkpoint,
        );
    }
    receipts.require_unattempted()?;
    let before = SystemObserver.redis_window(&inputs, selected, observed);
    let Ok(before) = before else {
        return receipts.unreadable(&checkpoint);
    };
    receipts.apply(&before, &checkpoint, |begin_write| {
        SystemObserver.redis_window(&inputs, selected, |command| {
            // 同一受管连接上绑定检查及唯一写入，命令前仍须具备持久意图。
            if observed(command)? != before {
                return Err("Redis 完整空前像在恢复命令前变化".into());
            }
            begin_write()?;
            let result = marker::rebind(&target.request, command)?;
            if result.as_str() != Some("2") {
                return Err("Redis 标记原子响应未确认".into());
            }
            observed(command)
        })
    })
}

fn empty_image() -> Value {
    json!({"keys":[],"owner":null,"sentinel":null})
}

fn protected_file(backend: &Path, path: &Path) -> Result<std::path::PathBuf> {
    let path = common::common_file(backend, path, "Redis 标记恢复绑定文件")?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if common::link_count_for(&path, &metadata)? != 1 {
        return Err("Redis 标记恢复绑定文件必须具有单一文件链接".into());
    }
    Ok(path)
}
