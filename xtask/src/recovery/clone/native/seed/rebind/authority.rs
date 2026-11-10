//! 当前原生控制器的能力上下文；构造只在正式重绑定阶段内进行。
use super::{classify, diagnostic::Phase, finish};
use crate::{
    Result,
    recovery::clone::native::{
        lifecycle::{identity, lock, stage::StageRequest, state},
        resources::common,
        source,
    },
};
use serde_json::Value;
use std::path::Path;

pub(crate) struct Controller<'a> {
    backend: &'a Path,
    run: &'a Path,
    number: u64,
    saved: Value,
    manifest: Value,
    state_binding: Value,
    owner: (Value, Value),
    lock_identity: (u64, u64),
    controller: Value,
    request: Value,
    tool: Value,
}

impl<'a> Controller<'a> {
    pub(super) fn load(request: &StageRequest<'a>, successor: &Value) -> Result<Self> {
        stage(request, true)?;
        let run = request.run_dir;
        common::local_directory(request.backend, run, "重绑定运行目录")?;
        let saved = state::load(run, true)?;
        let number = request.attempt.ok_or("重绑定缺少实际 attempt")?;
        let manifest = common::binding(&run.join("manifest.json"))?;
        let owner = lock::owner_observation(run)?.ok_or("重绑定缺少原运行锁")?;
        let controller = active(run, &saved, number, &manifest, &owner)?;
        let lock_path = run.join("run.lock");
        let value = Self {
            backend: request.backend,
            run,
            number,
            saved,
            manifest,
            state_binding: common::binding(&run.join("state.json"))?,
            owner,
            lock_identity: common::device_inode_for(
                &lock_path,
                &std::fs::symlink_metadata(&lock_path)?,
            )?,
            controller,
            request: successor.clone(),
            tool: crate::rate_limit::current_tool_binding()?,
        };
        value.checkpoint()?;
        Ok(value)
    }

    pub(crate) fn backend(&self) -> &Path {
        self.backend
    }
    pub(crate) fn run(&self) -> &Path {
        self.run
    }
    pub(crate) fn number(&self) -> u64 {
        self.number
    }
    pub(crate) fn saved(&self) -> &Value {
        &self.saved
    }
    pub(super) fn request(&self) -> &Value {
        &self.request
    }
    pub(super) fn tool(&self) -> &Value {
        &self.tool
    }

    pub(crate) fn checkpoint(&self) -> Result<()> {
        let manifest = common::binding(&self.run.join("manifest.json"))?;
        let saved = state::load(self.run, true)?;
        let owner = lock::owner_observation(self.run)?.ok_or("重绑定运行锁消失")?;
        let controller = active(self.run, &saved, self.number, &manifest, &owner)?;
        let path = self.run.join("run.lock");
        let lock_identity = common::device_inode_for(&path, &std::fs::symlink_metadata(&path)?)?;
        common::bound_file(self.backend, &self.request, "重绑定 successor 请求")?;
        if saved != self.saved
            || manifest != self.manifest
            || owner != self.owner
            || lock_identity != self.lock_identity
            || controller != self.controller
            || common::binding(&self.run.join("state.json"))? != self.state_binding
            || crate::rate_limit::current_tool_binding()? != self.tool
            || source::current(self.backend)?
                != state::attempts(&saved)?.last().ok_or("重绑定阶段消失")?["sources"]
        {
            return Err("重绑定期间完整账本、清单、锁、控制器、请求或原生工具来源变化".into());
        }
        Ok(())
    }

    pub(crate) fn checked<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        classify(Phase::Checkpoint, self.checkpoint())?;
        let result = operation();
        finish(result, classify(Phase::Checkpoint, self.checkpoint()))
    }
}

pub(super) fn stage(request: &StageRequest<'_>, executing: bool) -> Result<()> {
    if request.stage != "seed-runtime"
        || request.mode != "source-rebind"
        || if executing {
            request.attempt.is_none_or(|n| n == 0)
        } else {
            request.attempt.is_some()
        }
    {
        return Err("重绑定只接受正式 seed-runtime/source-rebind 当前阶段".into());
    }
    Ok(())
}

fn active(
    run: &Path,
    saved: &Value,
    number: u64,
    manifest: &Value,
    owner: &(Value, Value),
) -> Result<Value> {
    let attempt = state::running_attempt(saved)?.ok_or("重绑定缺少唯一当前阶段")?;
    if number == 0
        || attempt["number"].as_u64() != Some(number)
        || attempt["stage"] != "seed-runtime"
        || attempt["mode"] != "source-rebind"
        || attempt["status"] != "running"
        || state::attempts(saved)?.last() != Some(attempt)
    {
        return Err("重绑定 attempt 不是账本最后的唯一正式运行阶段".into());
    }
    identity::validate(&owner.1, run)?;
    if owner.1["identity"] != identity::current()?
        || owner.1["manifest_sha256"] != manifest["sha256"]
    {
        return Err("重绑定必须由同进程持有原清单的 RunLock".into());
    }
    let (controller, recorded) = state::controller_record(run, number, attempt)?;
    if recorded != owner.1 {
        return Err("重绑定阶段控制器不是当前原 RunLock owner".into());
    }
    Ok(controller)
}
