use serde_json::Value;

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

pub(crate) fn verify_with_observer(
    context: &TargetContext,
    observer: &impl StorageObserver,
) -> Result<()> {
    let review = review(context)?;
    let selected = generation::select(context)?;
    let rustfs = selected.storage.get("rustfs").ok_or("目标缺少 RustFS")?;
    let identity = common::validate_process_identity(&rustfs["identity"])?;
    rustfs::verify(
        context,
        &review,
        rustfs,
        selected.runtime.as_ref(),
        observer,
    )?;
    let redis = selected.storage.get("redis").ok_or("目标缺少 Redis")?;
    redis_layout::verify(context, &review, redis, observer)?;
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
