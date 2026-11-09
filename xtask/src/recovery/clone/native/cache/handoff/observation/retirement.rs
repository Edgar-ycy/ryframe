//! seed 交接发布后只允许原 post-copy API 退场，直到新运行首次尝试启动。

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        json as document,
        lifecycle::state,
        post_process::require_runtime_receipts,
        resources::common,
        seed::{self, history, runtime_model},
    },
};

pub(super) fn preflight(backend: &Path, run: &Path) -> Result<()> {
    if registration_path(backend, run)?.is_some() {
        require_prepared(&state::load(run, true)?)?;
    }
    Ok(())
}

pub(in crate::recovery::clone::native::cache::handoff) struct Retirement {
    runtime: PathBuf,
    directory_id: (u64, u64),
    prepare: Value,
    api: Value,
    tree: Value,
    bindings: Vec<Value>,
}

impl Retirement {
    pub(in crate::recovery::clone::native::cache::handoff) fn load(
        backend: &Path,
        run: &Path,
        post: &history::Post,
        saved: &Value,
    ) -> Result<Option<Self>> {
        let Some(registration) = registration_path(backend, run)? else {
            return Ok(None);
        };
        let prepare = require_prepared(saved)?["result"].clone();
        let registered = seed::Registration::load(backend, run, false)?;
        let active = history::post(
            backend,
            run,
            Some(&registered.value["post_copy"]),
            None,
            false,
        )?;
        if active.descriptor != post.descriptor || active.runtime != post.runtime {
            return Err("seed 原 API 退场只允许活动 post-copy 代次".into());
        }
        let inputs = runtime_model::inputs(backend, run)?;
        let handoff = &inputs.handoff;
        require_runtime_receipts(backend, &inputs.runtime)?;
        let directory_id = directory_identity(&inputs.runtime)?;
        let original_api =
            common::common_file(backend, &post.runtime.join("api.json"), "seed 原 API 进程")?;
        let api = common::binding(&original_api)?;
        let tree = common::binding(&common::common_file(
            backend,
            &post.runtime.join("api-tree.json"),
            "seed 原 API 监督树",
        )?)?;
        if registered.value["api_process"] != api {
            return Err("seed 登记不同于原 API 进程收据".into());
        }
        require_original(backend, post, handoff, &api)?;
        let handoff_path = inputs.runtime.join("handoff.json");
        let binding = common::binding(&handoff_path)?;
        let published = common::read_bound(backend, &prepare, "seed 最新准备阶段")?;
        if published["handoff"] != binding
            || common::read_bound(backend, &binding, "seed 退场交接")? != *handoff
            || handoff["registration"] != common::binding(&registration)?
            || state::load(run, true)? != *saved
        {
            return Err("seed 退场只接受最新完整准备阶段的原交接".into());
        }
        let bindings = vec![
            binding,
            handoff["registration"].clone(),
            handoff["runtime"].clone(),
            handoff["binaries"].clone(),
            prepare.clone(),
            api.clone(),
            tree.clone(),
        ];
        let value = Self {
            runtime: inputs.runtime,
            directory_id,
            prepare,
            api,
            tree,
            bindings,
        };
        value.check(backend)?;
        Ok(Some(value))
    }

    pub(super) fn paths(&self, backend: &Path) -> Result<Vec<PathBuf>> {
        self.bindings
            .iter()
            .map(|file| common::bound_file(backend, file, "seed 原 API 退场固定收据"))
            .collect()
    }

    pub(super) fn check(&self, backend: &Path) -> Result<()> {
        require_runtime_receipts(backend, &self.runtime)?;
        if directory_identity(&self.runtime)? != self.directory_id {
            return Err("seed 运行目录在原 API 退场期间变化".into());
        }
        self.paths(backend)?;
        Ok(())
    }

    pub(super) fn evidence(&self) -> Value {
        json!({"prepare":self.prepare,"api_process":self.api,"api_tree":self.tree})
    }
}

fn require_original(
    backend: &Path,
    post: &history::Post,
    handoff: &Value,
    api: &Value,
) -> Result<()> {
    let original = common::read_bound(backend, api, "seed 原 API 进程")?;
    document::exact(
        &original,
        &["format_version", "role", "scope_id", "identity"],
        "seed 原 API 收据",
    )?;
    common::validate_process_identity(&original["identity"])?;
    let old = document::read(&post.runtime.join("handoff.json"))?;
    let runtime = common::read_bound(backend, &handoff["runtime"], "seed 运行合同")?;
    let binaries = common::read_bound(backend, &handoff["binaries"], "seed 二进制清单")?;
    if handoff["original_api"] != *api
        || handoff["backend_build"] != post.request["backend_build"]
        || original["format_version"] != 1
        || original["role"] != "api"
        || original["scope_id"] != handoff["scope_id"]
        || runtime["scope_id"] != handoff["scope_id"]
        || original["identity"]["executable"] != runtime["artifacts"]["api"]["path"]
        || old["registration"] != post.descriptor
        || old["api_url"] != handoff["api_url"]
        || runtime != common::read_bound(backend, &old["runtime"], "原 post-copy 运行合同")?
        || binaries != common::read_bound(backend, &old["binaries"], "原 post-copy 二进制清单")?
    {
        return Err("seed 退场交接未固定同一原 API、产品或作用域".into());
    }
    crate::recovery::full_stack::worker::controller::require_api_tree(&post.runtime, &runtime)
}

fn require_prepared(saved: &Value) -> Result<&Value> {
    let mut stages = state::attempts(saved)?
        .iter()
        .filter(|row| row["stage"] == "seed-runtime");
    if stages
        .clone()
        .any(|row| matches!(row["mode"].as_str(), Some("start" | "close")))
    {
        return Err("新 seed 已有运行尝试，禁止再控制原 post-copy API".into());
    }
    stages
        .rfind(|row| {
            !matches!(
                row["mode"].as_str(),
                Some("status" | "recover" | "recover-session")
            )
        })
        .filter(|row| row["mode"] == "prepare" && row["status"] == "passed")
        .ok_or_else(|| "原 API 退场须先完成最新 seed prepare".into())
}

fn registration_path(backend: &Path, run: &Path) -> Result<Option<PathBuf>> {
    let file = run.join("seed-runtime.json");
    match fs::symlink_metadata(&file) {
        Ok(_) => Ok(Some(common::common_file(
            backend,
            &file,
            "原 API 退场 seed 登记",
        )?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn directory_identity(path: &Path) -> Result<(u64, u64)> {
    common::device_inode_for(path, &fs::symlink_metadata(path)?)
}
