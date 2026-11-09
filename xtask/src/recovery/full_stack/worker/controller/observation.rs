//! 只读核验原角色合同、监听和完整进程树。
use std::{path::Path, time::Duration};

use serde_json::{Value, json};

use super::{Role, free, operation, process, ready, tree};
use crate::Result;

pub(crate) fn observe(role: Role, backend: &Path, directory: &Path) -> Result<Value> {
    let (identity, _) = observe_role(role, backend, directory, Readiness::Required)?;
    Ok(json!({"state":if identity.is_some(){"running"}else{"stopped"},"identity":identity}))
}

#[derive(Clone, Copy)]
pub(super) enum Readiness {
    Required,
    Probe,
    Skip,
}

impl Readiness {
    pub(super) fn sample(
        self,
        role: Role,
        probe: impl FnOnce() -> Result<bool>,
        listener: impl FnOnce() -> Result<()>,
    ) -> Result<bool> {
        if matches!(self, Self::Skip) {
            return Ok(false);
        }
        let ready = probe()?;
        if matches!(self, Self::Required) && !ready {
            return Err(
                super::super::Rejection(format!("已登记{}未通过就绪探针", role.label())).into(),
            );
        }
        listener()?;
        Ok(ready)
    }
}

pub(super) fn observe_role(
    role: Role,
    backend: &Path,
    directory: &Path,
    readiness: Readiness,
) -> Result<(Option<Value>, bool)> {
    if role == Role::Frontend {
        return Err("前端有限命令使用完成证明观察".into());
    }
    let runtime = super::super::runtime::load(role, backend, directory).map_err(|_| {
        super::super::Rejection(format!(
            "{}运行合同核验失败；核对冻结来源、配置与产物，保留现场且不启动或停止进程",
            role.label()
        ))
    })?;
    let source = operation::source(directory)?;
    let tree = tree::read(
        directory,
        &runtime,
        role,
        directory.join(format!("{}.json", role.name())).exists(),
    )?;
    let identity = process::registered(role, directory, &runtime)?;
    let ready = if let Some(identity) = &identity {
        observe_running(role, directory, &runtime, identity, readiness)?
    } else {
        if let Some(tree) = &tree {
            tree::closure(tree, Duration::from_secs(5), false)?;
        }
        free(role, &runtime, 0.0)?;
        false
    };
    if super::super::runtime::load(role, backend, directory)? != runtime
        || operation::source(directory)? != source
    {
        return Err("角色运行输入在只读观察期间漂移".into());
    }
    Ok((identity, ready))
}
pub(super) fn verify_running(
    role: Role,
    directory: &Path,
    runtime: &Value,
    identity: &Value,
) -> Result<()> {
    observe_running(role, directory, runtime, identity, Readiness::Required).map(|_| ())
}

fn observe_running(
    role: Role,
    directory: &Path,
    runtime: &Value,
    identity: &Value,
    readiness: Readiness,
) -> Result<bool> {
    let url = runtime[role.ready()].as_str().ok_or("端点无效")?;
    let ready = readiness.sample(role, || ready(url), || process::listener(identity, url))?;
    tree::running(directory, runtime, role, identity)?;
    Ok(ready)
}
