//! 工作区与 storage run 的固定输入窗口；锁释放失败与主错误同时保留。
use super::super::super::{
    json as document,
    lifecycle::lock::{ProcessGuard, RunLock},
    resources::common,
};
use super::{
    super::{binding, cli},
    storage,
};
use crate::Result;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) struct Registration {
    pub(super) backend: PathBuf,
    pub(super) workspace: PathBuf,
    pub(super) target: PathBuf,
    pub(super) value: Value,
    pub(super) binding: Value,
    pub(super) environment: Value,
    pub(super) storage: PathBuf,
    storage_state: Value,
}

impl Registration {
    pub(crate) fn load(backend: &Path, workspace: &Path) -> Result<Self> {
        let observed = cli::execute(
            backend,
            workspace,
            "cli-registration",
            &serde_json::json!({}),
        )?;
        let value = &observed["value"];
        let environment =
            binding::private_environment(backend, &value["environment"])?["environment"].clone();
        let storage = storage::load(backend, &value["storage_run"])?;
        let result = Self {
            backend: backend.to_owned(),
            workspace: workspace.to_owned(),
            target: PathBuf::from(document::text(
                &value["target_directory"],
                "fresh 目标目录",
            )?),
            value: value.clone(),
            binding: observed["binding"].clone(),
            environment,
            storage_state: common::binding(&storage.join("state.json"))?,
            storage,
        };
        result.check(true)?;
        Ok(result)
    }

    pub(super) fn check(&self, state: bool) -> Result<()> {
        let filename = self.workspace.join("registration.json");
        if common::binding(&filename)? != self.binding
            || document::read(&filename)? != self.value
            || binding::private_environment(&self.backend, &self.value["environment"])?["environment"]
                != self.environment
        {
            return Err("fresh 登记或私有环境在阶段期间变化".into());
        }
        common::bound_file(&self.backend, &self.value["request"], "fresh 固定请求")?;
        if storage::load(&self.backend, &self.value["storage_run"])? != self.storage
            || state && common::binding(&self.storage.join("state.json"))? != self.storage_state
        {
            return Err("fresh 存储运行在阶段期间变化".into());
        }
        Ok(())
    }

    pub(crate) fn execute<T>(
        &self,
        operation: impl FnOnce(Option<&Path>) -> Result<T>,
    ) -> Result<T> {
        let availability =
            storage::availability(&self.backend, &self.storage, &self.value["storage_run"])?;
        if !availability.is_null() && availability["available"] != true {
            return Err(document::text(&availability["reason"], "服务不可用原因")?
                .to_owned()
                .into());
        }
        let active = if availability["active_generation"]["kind"] == "initial" {
            None
        } else {
            Some(self.storage.as_path())
        };
        let request = common::read_bound(&self.backend, &self.value["request"], "fresh 请求")?;
        super::super::storage::redis_layout::require_windows(&request["storage"]["redis"])?;
        crate::recovery::fixture_control::review::reconciled::validate_request(
            &self.backend,
            &request,
            &self.storage,
        )?;
        let binaries = super::super::binding_request::binaries(&self.backend, &request)?;
        let paths = document::array(&binaries["bindings"], "fresh 二进制")?
            .iter()
            .map(super::super::input::request::external_file)
            .collect::<Result<Vec<_>>>()?;
        let binary_guard = super::super::guard::Inputs::open(paths)?;
        let guard = ProcessGuard::acquire(&self.storage)?;
        let lock = RunLock::claim(&self.storage)?;
        let result = (|| {
            self.check(true)?;
            let result = operation(active);
            combine(combine(result, self.check(true)), binary_guard.check())
        })();
        combine(combine(result, lock.release()), guard.check())
    }
}

pub(crate) struct Workspace {
    path: PathBuf,
    identity: (u64, u64),
    guard: ProcessGuard,
}

impl Workspace {
    pub(crate) fn claim(backend: &Path, workspace: &Path) -> Result<Self> {
        common::local_directory(backend, workspace, "fresh 工作区")?;
        let identity = common::device_inode_for(workspace, &std::fs::symlink_metadata(workspace)?)?;
        let guard = ProcessGuard::acquire_named(workspace, "fresh-target.guard")?;
        let value = Self {
            path: workspace.to_owned(),
            identity,
            guard,
        };
        value.check()?;
        Ok(value)
    }
    pub(super) fn check(&self) -> Result<()> {
        let metadata = std::fs::symlink_metadata(&self.path)?;
        if !metadata.is_dir()
            || crate::recovery::clone::native::digest::linked(&metadata)
            || common::device_inode_for(&self.path, &metadata)? != self.identity
        {
            return Err("fresh 工作区在阶段期间被替换".into());
        }
        self.guard.check()
    }
    pub(crate) fn finish<T>(self, result: Result<T>) -> Result<T> {
        combine(result, self.check())
    }
}

pub(super) fn combine<T>(result: Result<T>, check: Result<()>) -> Result<T> {
    match (result, check) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(check)) => {
            Err(format!("{error}；固定输入或锁收尾同时失败：{check}").into())
        }
    }
}
