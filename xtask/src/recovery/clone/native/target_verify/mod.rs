use std::{fs, path::Path};

use serde_json::{Value, json};

use super::lifecycle::stage::{StageRequest, StageRunner};
use super::{digest, export::transport::SystemTransport, json as json_io};
use crate::Result;

pub(crate) mod authority;
pub(crate) mod database;
mod execution;
pub(crate) mod guard;
pub(crate) mod input;
mod object;
pub(crate) mod proof;
pub(crate) mod publication;
pub(crate) mod redis;
pub(crate) mod reset;
pub(crate) mod storage;
pub(crate) mod tree;

pub(crate) fn preflight(backend: &Path, request: &StageRequest<'_>) -> Result<()> {
    preflight_with(backend, request, &authority::SystemObserver)
}

pub(crate) fn preflight_with(
    backend: &Path,
    request: &StageRequest<'_>,
    observer: &impl authority::Observer,
) -> Result<()> {
    input::validate_inputs(backend, request)?;
    let _ = authority::inspect(backend, request, observer)?;
    Ok(())
}

pub(crate) fn execute(backend: &Path, request: &StageRequest<'_>) -> Result<Value> {
    let transport = SystemTransport;
    execute_with(
        backend,
        request,
        &transport,
        &storage::SystemObserver::new(&transport),
        &authority::SystemObserver,
    )
}

pub(crate) fn execute_with<T: super::export::transport::Transport>(
    backend: &Path,
    request: &StageRequest<'_>,
    transport: &T,
    storage_observer: &impl storage::StorageObserver,
    authority_observer: &impl authority::Observer,
) -> Result<Value> {
    let context = input::TargetContext::load(backend, request)?;
    let authority = authority::inspect(backend, request, authority_observer)?;
    let frame = execution::Frame::begin(&context, authority)?;
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
        verify(&context, &transport, &observer, &|| frame.check())
    })();
    let result = frame.finish(result, authority_observer);
    match result {
        Ok(value) => {
            if let Err(error) =
                publication::publish(&context.observation_dir.join("verify.json"), &value)
            {
                return fail(&context, error);
            }
            Ok(value)
        }
        Err(error) => fail(&context, error),
    }
}

fn fail(context: &input::TargetContext, error: Box<dyn std::error::Error>) -> Result<Value> {
    let secrets = context
        .environment
        .iter()
        .filter(|(key, value)| {
            !value.is_empty()
                && ["KEY", "PASSWORD", "PASSWD", "SECRET", "TOKEN", "CREDENTIAL"]
                    .iter()
                    .any(|part| key.contains(part))
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let reason = super::export::transport::redact(&error.to_string(), &secrets);
    let path = context.observation_dir.join("failure.json");
    if !context.observation_dir.is_dir() {
        return Err(format!(
            "目标复核未完成：{reason}；观察目录尚未建立，核对运行 attempt 证据；不自动重放或清理"
        )
        .into());
    }
    json_io::write_new(&path, &json!({
        "status": "needs_reconciliation", "stage": "verify_existing",
        "at": chrono::Utc::now().to_rfc3339(), "error_type": "TargetVerifyError", "reason": reason,
        "automatic_retry": false, "automatic_resource_cleanup": false,
        "fresh_target_initialized": false, "clone_verified": false, "restore_qualified": false,
    })).map_err(|write| format!("目标复核未完成：{reason}；失败收据保存同时失败：{write}；保留现场，不自动重放或清理"))?;
    Err(format!(
        "目标复核未完成：{reason}；诊断保留在 {}；核对原证据后显式选择后续操作，不自动重放或清理",
        path.display()
    )
    .into())
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
    transport: &T,
    observer: &impl storage::StorageObserver,
    checkpoint: &impl Fn() -> Result<()>,
) -> Result<Value> {
    proof::verify_history(context)?;
    proof::verify_initial_inventory(context)?;
    proof::verify_reset(context)?;
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
    let redis = redis::verify_with_observer(
        context,
        &context.initialized["generation"]["storage"]["redis"],
        observer,
    )?;
    if redis != context.initialized["redis"] {
        return Err("目标当前 Redis 状态与初始化像不同".into());
    }
    storage::verify_with_observer(context, observer)?;
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
