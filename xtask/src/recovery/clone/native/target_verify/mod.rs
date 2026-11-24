use std::{fs, path::Path};

use serde_json::{Value, json};

use super::lifecycle::stage::{StageRequest, StageRunner};
use super::{
    cache::handoff::locks::Storage, digest, export::transport::SystemTransport, json as json_io,
};
use crate::Result;
use crate::utc_timestamp::now_rfc3339;

pub(crate) mod authority;
mod binding;
mod binding_configuration;
pub(crate) mod binding_request;
mod cli;
mod creation_plan;
pub(crate) mod database;
mod diagnostic;
pub(crate) mod evidence_creation;
mod evidence_model;
pub(crate) mod evidence_prepare;
pub(crate) mod evidence_protocol;
pub(crate) mod evidence_reset;
mod execution;
pub(crate) mod fresh;
pub(crate) mod guard;
pub(crate) mod input;
mod object;
mod prepare_groups;
pub(crate) mod prepare_observations;
pub(crate) mod prepare_state;
pub(crate) mod proof;
pub(crate) mod publication;
pub(crate) mod published;
pub(crate) mod redis;
pub(crate) mod reset;
mod resume;
pub(crate) mod resume_baseline;
mod resume_confirmed;
mod resume_model;
mod resume_observations;
mod resume_predecessor;
pub(crate) mod resume_prefix;
mod resume_runtime;
mod resume_state;
#[cfg(test)]
#[path = "../../../../../tests/internal/target_resume_evidence.rs"]
mod resume_tests;
pub(crate) mod storage;
pub(crate) mod tree;

pub(crate) use diagnostic::diagnostic;
#[cfg(not(test))]
use execution::execute_context;
#[cfg(test)]
pub(crate) use execution::execute_context;

pub(crate) fn preflight(backend: &Path, request: &StageRequest<'_>) -> Result<()> {
    input::validate_live_inputs(backend, request)?;
    authority::inspect(backend, request, &authority::SystemObserver)?;
    Ok(())
}

pub(crate) fn execute(backend: &Path, request: &StageRequest<'_>) -> Result<Value> {
    let context = input::TargetContext::load(backend, request)?;
    storage::redis_layout::require_windows(&context.storage()["redis"])?;
    execute_context(
        &context,
        request,
        &SystemTransport,
        &storage::SystemObserver,
        &authority::SystemObserver,
    )
}

fn verify_and_publish<T: super::export::transport::Transport>(
    context: &input::TargetContext,
    request: &StageRequest<'_>,
    transport: &T,
    storage_observer: &impl storage::StorageObserver,
    authority_observer: &impl authority::Observer,
    external_storage: Option<&Storage>,
) -> Result<Value> {
    let authority = authority::inspect(&context.backend, request, authority_observer)?;
    let frame = execution::Frame::begin(context, authority)?;
    let result = (|| {
        fs::create_dir(&context.observation_dir)
            .map_err(|error| format!("无法创建目标观察目录：{error}"))?;
        let transport = execution::CheckedTransport {
            inner: transport,
            frame: &frame,
        };
        let observer = execution::CheckedObserver {
            inner: storage_observer,
            frame: &frame,
        };
        let owner = frame.owner()?;
        let selected = storage::generation::select(context, owner)?;
        let observed = verify(context, owner, &selected, &transport, &observer, &|| {
            frame.checkpoint()
        });
        execution::combine(observed, selected.check(context)).map(|value| (value, selected))
    })();
    let result = frame.finish(result, authority_observer);
    match result {
        Ok((value, selected)) => {
            let publication = (|| {
                selected.check(context)?;
                if let Some(storage) = external_storage {
                    storage.check()?;
                }
                publication::publish(&context.observation_dir.join("verify.json"), &value)
            })();
            match publication {
                Ok(()) => Ok(value),
                Err(error) => fail(context, error),
            }
        }
        Err(error) => fail(context, error),
    }
}

fn fail(context: &input::TargetContext, error: Box<dyn std::error::Error>) -> Result<Value> {
    let reason = diagnostic::reason(error.as_ref());
    let path = context.observation_dir.join("failure.json");
    if !context.observation_dir.is_dir() {
        return Err(format!(
            "目标复核未完成：{reason}；观察目录尚未建立，核对运行 attempt 证据；不自动重放或清理"
        )
        .into());
    }
    json_io::write_new(&path, &json!({
        "status": "needs_reconciliation", "stage": "verify_existing",
        "at": now_rfc3339(), "error_type": "TargetVerifyError", "reason": reason,
        "automatic_retry": false, "automatic_resource_cleanup": false,
        "fresh_target_initialized": false, "clone_verified": false, "restore_qualified": false,
    })).map_err(|_| format!("目标复核未完成：{reason}；失败收据保存同时失败；保留现场并核对运行 attempt 证据，不自动重放或清理"))?;
    Err(diagnostic::saved(error, &path, &context.run_dir))
}

pub(crate) struct TargetVerifyRunner {
    backend: std::path::PathBuf,
}

impl TargetVerifyRunner {
    pub(crate) fn new(backend: &Path) -> Self {
        Self {
            backend: backend.to_path_buf(),
        }
    }
}

impl StageRunner for TargetVerifyRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        preflight(&self.backend, request)
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        execute(&self.backend, request)
    }
}

fn verify<T: super::export::transport::Transport>(
    context: &input::TargetContext,
    owner: &guard::Generation,
    selected: &storage::generation::Selected,
    transport: &T,
    observer: &impl storage::StorageObserver,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<Value> {
    proof::verify_history(context)?;
    proof::verify_initial_inventory(context)?;
    proof::verify_reset(context, owner)?;
    proof::verify_generation(context)?;
    let captured = database::capture_with(context, transport, checkpoint)?;
    let expected_inventory = context
        .initialized
        .pointer("/inventory/observations")
        .ok_or("目标初始化收据缺少 inventory observations")?;
    let observed_inventory = Value::Object(captured.observations.clone().into_iter().collect());
    if observed_inventory != *expected_inventory {
        return Err("目标当前数据库库存与初始化像不同".into());
    }
    let objects = object::verify(context, transport)?;
    if objects != context.initialized["objects"] {
        return Err("目标当前对象状态与初始化像不同".into());
    }
    let redis = redis::verify_with_observer(context, &selected.storage["redis"], observer)?;
    if redis != context.initialized["redis"] {
        return Err("目标当前 Redis 状态与初始化像不同".into());
    }
    storage::verify_with_observer(context, selected, observer)?;
    if tree::snapshot(
        &context.backend,
        &context.observation_dir.join("inventory"),
        false,
    )? != json!(captured.evidence_files)
    {
        return Err("库存采集后的完整证据集合发生变化".into());
    }
    let initialized = digest::file(&context.initialized_path)?;
    if context.initialized_binding["bytes"].as_u64() != Some(initialized.bytes)
        || context.initialized_binding["sha256"].as_str() != Some(initialized.sha256.as_str())
    {
        return Err("目标 initialized 发布绑定在核验期间变化".into());
    }
    Ok(json!({
        "status": "fresh_target_reverified",
        "initialized": context.initialized_binding,
        "inventory": {
            "receipt": captured.receipt,
            "observations": captured.observations,
        },
        "remote_writes": 0,
        "target_ready": false,
        "clone_verified": false,
        "restore_qualified": false,
    }))
}
