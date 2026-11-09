use std::path::Path;

use serde_json::Value;

use super::lifecycle::{manifest, stage::StageRequest};
use crate::Result;

pub(crate) mod capture;
pub(crate) mod database;
mod evidence;
pub(crate) mod input;
pub(crate) mod inventory;
mod inventory_validate;
pub(crate) mod object;
pub(crate) mod object_owner;
pub(crate) mod proof;
pub(crate) mod recovery;
pub(crate) mod transport;
mod verify;

pub(crate) use evidence::bound_relative;
pub(crate) use verify::SavedObjectInputs;
pub(crate) use verify::frozen;

pub(crate) fn preflight(backend: &Path, request: &StageRequest<'_>) -> Result<()> {
    let _ = input::Context::load(backend, request)?;
    if request.attempt.is_some() {
        return Err("源导出预检不得携带阶段序号".into());
    }
    let loaded = manifest::load(backend, &request.run_dir.join("manifest.json"))?;
    if loaded.value["source_export"].is_null() && request.mode == "run" {
        recovery::reject_reexport(backend, request.run_dir, 0)?;
    }
    Ok(())
}

pub(crate) fn execute(backend: &Path, request: &StageRequest<'_>) -> Result<Value> {
    if request.attempt.is_none() {
        return Err("源导出执行缺少阶段序号".into());
    }
    let context = input::Context::load(backend, request)?;
    match context.mode.as_str() {
        "run" => capture::run(&context),
        "reconcile" => recovery::reconcile(&context),
        "resume" => recovery::resume(&context),
        _ => Err("源导出模式无效".into()),
    }
}
