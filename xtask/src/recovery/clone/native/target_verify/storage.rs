use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use super::super::{
    export::transport::Transport,
    json::{self as json_io, exact},
    resources::common,
};
use super::input::TargetContext;
use crate::Result;

pub(crate) mod generation;
pub(crate) mod kernel;
mod observer;
pub(crate) mod redis_layout;
pub(crate) mod rustfs;

pub(crate) use observer::{StorageObserver, SystemObserver};

/// 初始化前后共用的真实只读输入，不携带尚未产生的初始化收据。
pub(crate) struct StorageInputs<'a> {
    pub(crate) backend: &'a Path,
    pub(crate) request: &'a Value,
    pub(crate) environment: &'a BTreeMap<String, String>,
    pub(crate) observation_dir: &'a Path,
}

impl StorageInputs<'_> {
    pub(crate) fn execution_backend(&self) -> Result<PathBuf> {
        match self
            .request
            .get("execution_backend")
            .filter(|value| !value.is_null())
        {
            Some(value) => Ok(PathBuf::from(json_io::text(
                &value["path"],
                "目标执行工作树",
            )?)),
            None => Ok(self.backend.to_path_buf()),
        }
    }
}

pub(crate) fn verify_with_observer(
    context: &TargetContext,
    observer: &impl StorageObserver,
) -> Result<()> {
    let review = review(context)?;
    let selected = generation::select(context)?;
    let rustfs = selected.storage.get("rustfs").ok_or("目标缺少 RustFS")?;
    let identity = common::validate_process_identity(&rustfs["identity"])?;
    let inputs = context.storage_inputs();
    rustfs::verify(
        &inputs,
        &review,
        rustfs,
        selected.runtime.as_ref(),
        observer,
    )?;
    let redis = selected.storage.get("redis").ok_or("目标缺少 Redis")?;
    redis_layout::verify(&inputs, &review, redis, observer)?;
    kernel::require_observed_identity(&identity, observer)
}

fn review(context: &TargetContext) -> Result<Value> {
    let binding = context.request.get("review").ok_or("目标缺少审阅计划")?;
    let mut descriptor = json_io::object(binding, "目标审阅绑定")?.clone();
    let canonical = descriptor
        .remove("canonical_sha256")
        .ok_or("目标审阅绑定缺少规范摘要")?;
    let review = common::read_bound(&context.backend, &Value::Object(descriptor), "目标审阅计划")?;
    if canonical.as_str() != Some(super::super::digest::plan_hash(&review)?.as_str()) {
        return Err("目标审阅计划规范摘要不同".into());
    }
    exact(
        &context.initialized["generation"]["storage"],
        &["rustfs", "redis"],
        "目标存储代次",
    )?;
    Ok(review)
}
