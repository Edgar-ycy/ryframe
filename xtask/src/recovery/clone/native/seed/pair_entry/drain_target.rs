//! 排空读取绑定当前父控制器持有的目标锁及原初始化代次。

use std::{fs, path::Path};

use serde_json::Value;

use super::{Frame, drain};
use crate::{
    Result,
    recovery::clone::native::{
        digest, json as document,
        lifecycle::state,
        post_process::files,
        resources::common,
        target_verify::{guard::Inputs, published, storage::StorageInputs},
    },
};

pub(super) fn observe(frame: &Frame<'_>, inode: u64) -> Result<Value> {
    let backend = frame.backend;
    let manifest = common::read_bound(backend, &state::manifest_binding(frame.run)?, "原运行清单")?;
    let initialized_path = common::bound_file(backend, &manifest["initialized"], "原目标初始化")?;
    let target = initialized_path
        .parent()
        .ok_or("目标初始化文件缺少父目录")?;
    if initialized_path != target.join("initialized.json") {
        return Err("seed 排空必须读取固定目标初始化文件".into());
    }
    let lock = common::local_directory(backend, &target.join("initialize.lock"), "seed 目标锁")?;
    check_lock(&lock, inode)?;

    let prepare_path = common::common_file(backend, &target.join("prepare.json"), "原目标准备")?;
    let prepare = common::binding(&prepare_path)?;
    let prepared = common::read_bound(backend, &prepare, "原目标准备")?;
    let request_path = common::bound_file(backend, &prepared["request"], "原目标请求")?;
    let fixed = common::common_file(backend, &target.join("request.json"), "固定目标请求")?;
    let candidate = common::common_file(
        backend,
        &target.join("initialized-candidate.json"),
        "原目标初始化候选",
    )?;
    let protected = Inputs::open([
        initialized_path,
        prepare_path,
        request_path,
        fixed.clone(),
        candidate.clone(),
    ])?;
    let initialized = common::read_bound(backend, &manifest["initialized"], "原目标初始化")?;
    let request = common::read_bound(backend, &prepared["request"], "原目标请求")?;
    let selection = published::request_binding(backend, &request)?;
    if initialized["status"] != "fresh_target_initialized"
        || initialized["controlled_generation_never_started"] != true
        || initialized["prepare_sha256"] != prepare["sha256"]
        || initialized["generation"] != prepared["generation"]
        || initialized["id"] != request["id"]
        || prepared["request_sha256"].as_str() != Some(digest::plan_hash(&request)?.as_str())
        || initialized["scope_id"] != frame.runtime.handoff["scope_id"]
        || request["target"]["scope_id"] != initialized["scope_id"]
        || selection["selected"]["scope_id"] != initialized["scope_id"]
        || document::read(&fixed)? != request
        || document::read(&candidate)? != initialized
    {
        return Err("seed 排空请求不同于原初始化及固定 seed 代次".into());
    }
    let output = common::local_directory(
        backend,
        &frame
            .run
            .join(format!("seed-runtime/attempt-{:04}", frame.number)),
        "seed 关闭诊断目录",
    )?;
    let environment = serde_json::from_value(frame.runtime.environment.clone())?;
    let storage = StorageInputs {
        backend,
        request: &request,
        environment: &environment,
        observation_dir: &output,
    };
    let checkpoint = || {
        frame
            .guard
            .check_named(&frame.run.join("seed-runtime"), "runtime-control.guard")?;
        frame.protected.checkpoint()?;
        protected.checkpoint()?;
        check_lock(&lock, inode)
    };
    checkpoint()?;
    let observed = drain::observe(&storage, &checkpoint);
    files::combine(
        observed,
        (|| {
            checkpoint()?;
            protected.check()?;
            frame.check()
        })(),
    )
}

fn check_lock(lock: &Path, inode: u64) -> Result<()> {
    let metadata = fs::symlink_metadata(lock)?;
    if digest::linked(&metadata)
        || !metadata.is_dir()
        || common::device_inode_for(lock, &metadata)?.1 != inode
        || fs::read_dir(lock)?.next().is_some()
    {
        return Err("seed 排空目标锁已经变化或属于复制控制器".into());
    }
    Ok(())
}
