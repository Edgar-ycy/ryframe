//! DevEx 原生测量器；仅接收控制器绑定的输入，不读取凭据环境变量。

pub(crate) mod artifacts;
pub(crate) mod configuration;
pub(crate) mod entry;
pub(crate) mod failure;
pub(crate) mod homepage;
pub(crate) mod imports;
pub(crate) mod jobs;
pub(crate) mod load;
mod mysql;
pub(crate) mod pacing;
pub(crate) mod process;
pub(crate) mod provenance;
pub(crate) mod request;
pub(crate) mod telemetry;

#[cfg(test)]
#[path = "../../../tests/internal/devex_runtime_native.rs"]
mod tests;

use failure::{Failure, NativeResult};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};

/// 所有来源由控制器显式传入；进程环境不构成第二套输入。
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Input {
    pub backend: PathBuf,
    pub frontend: PathBuf,
    pub runner_frontend: PathBuf,
    pub driver: PathBuf,
    pub output: PathBuf,
    pub suite: String,
    pub cache: String,
    pub concurrency: u64,
    pub input_sha256: String,
    pub driver_fingerprint: String,
    pub runner_frontend_fingerprint: String,
    pub source_fingerprints: Value,
    pub app_env: String,
}

impl Input {
    pub(crate) fn validate(&self) -> NativeResult<()> {
        if !matches!(self.suite.as_str(), "homepage" | "api" | "jobs" | "tenants")
            || !matches!(self.cache.as_str(), "cold" | "warm")
            || (self.suite != "homepage"
                && (!matches!(self.concurrency, 10 | 50 | 100) || self.cache != "warm"))
            || !matches!(self.app_env.as_str(), "dev" | "test")
            || !configuration::digest(&self.input_sha256)
            || !configuration::fingerprint(&self.driver_fingerprint)
            || !configuration::fingerprint(&self.runner_frontend_fingerprint)
            || [
                &self.backend,
                &self.frontend,
                &self.runner_frontend,
                &self.driver,
                &self.output,
            ]
            .iter()
            .any(|path| !path.is_absolute())
        {
            return Err(Failure::new("runtime_parameters"));
        }
        Ok(())
    }
}

/// 返回原公开输出与退出码；配置变化或来源变化时不发布成功收据。
pub(crate) async fn execute(input: &Input, cancel: &process::Cancellation) -> NativeResult<i32> {
    input.validate()?;
    let path = input.backend.join(".local-tests/devex/runtime.json");
    let before = artifacts::read(&path, 16 * 1024 * 1024)?;
    let config = configuration::read(&before, input)?;
    let directory = PathBuf::from(format!("{}.artifacts", input.output.display()));
    artifacts::directory(&directory)?;
    let selection = configuration::selection_config(&config)?;
    let receipt =
        super::runtime_selection::validate_suite(&selection, &input.suite, input.concurrency)
            .map_err(|_| Failure::new("identity_selection"))?;
    artifacts::write(&directory.join("identity-selection.json"), &receipt)?;
    let mut provenance = provenance::Guard::before(&config, input, &directory, cancel).await?;
    let mut native = jobs::ArtifactGuard::before(&config, &input.suite, &directory)?;
    let pacing = pacing::Pacing::new(&config, input, &directory);
    let mut pacing = match pacing {
        Ok(pacing) => pacing,
        Err(error) => {
            let verified = provenance.after(cancel).await;
            let guarded = native.after();
            return merge::<()>(Err(error), [verified, guarded]).map(|()| 1);
        }
    };
    let measured = measure(&config, input, &directory, &mut pacing, cancel).await;
    let closed = pacing.close();
    let verified = provenance.after(cancel).await;
    let guarded = native.after();
    // 全部清理与后像核验都必须执行，原始失败优先保留。
    let output = merge(measured, [closed, verified, guarded])?;
    publish(input, &before, &output, cancel)
}

pub(crate) fn publish(
    input: &Input,
    before: &[u8],
    output: &Value,
    cancel: &process::Cancellation,
) -> NativeResult<i32> {
    if artifacts::read(
        &input.backend.join(".local-tests/devex/runtime.json"),
        16 * 1024 * 1024,
    )? != before
    {
        return Err(Failure::new("runtime_configuration_changed"));
    }
    cancel.check()?;
    let failed = output["collector_failures"].as_u64().unwrap_or(1) != 0
        || output["failed_cycles"].as_u64().unwrap_or(1) != 0
        || output["session_failures"].as_u64().unwrap_or(1) != 0
        || output["scenarios"].as_object().is_none_or(|rows| {
            rows.values().any(|row| {
                row["failed"].as_u64().unwrap_or(1) != 0
                    || row["completed"].as_u64().unwrap_or(0) == 0
            })
        });
    artifacts::write(&input.output, &output)?;
    Ok(i32::from(failed))
}

pub(crate) fn merge<T>(
    result: NativeResult<T>,
    cleanup: impl IntoIterator<Item = NativeResult<()>>,
) -> NativeResult<T> {
    let mut value = None;
    let mut failure = match result {
        Ok(result) => {
            value = Some(result);
            None
        }
        Err(error) => Some(error),
    };
    for result in cleanup {
        if let Err(error) = result {
            failure = Some(match failure {
                Some(previous) => previous.and(error),
                None => error,
            });
        }
    }
    if let Some(failure) = failure {
        Err(failure)
    } else {
        value.ok_or_else(|| Failure::new("runtime_result_missing"))
    }
}

async fn measure(
    config: &Value,
    input: &Input,
    directory: &std::path::Path,
    pacing: &mut pacing::Pacing,
    cancel: &process::Cancellation,
) -> NativeResult<Value> {
    pacing.prepare_sample(cancel).await?;
    let samples = imports::prepare(config, &input.suite, input.concurrency, directory, cancel)?;
    let catalog = configuration::catalog(&input.backend)?;
    let observer = telemetry::Observer::start(config, cancel.clone()).await?;
    let result = if input.suite == "homepage" {
        homepage::measure(config, input, directory, cancel).await
    } else {
        load::run(
            config,
            &catalog,
            input,
            directory,
            samples.as_ref(),
            pacing,
            cancel,
        )
        .await
    };
    let resources = observer.stop().await;
    let mut measured = result?;
    let resources = resources?;
    // 资源观察结束后才读取全部尝试历史。
    measured.finalize(config, directory, cancel).await?;
    let mut document = measured.document();
    let object = document
        .as_object_mut()
        .ok_or_else(|| Failure::new("runtime_output"))?;
    object.extend(
        resources
            .as_object()
            .ok_or_else(|| Failure::new("runtime_output"))?
            .clone(),
    );
    object.insert("input_sha256".into(), json!(input.input_sha256));
    object.insert("cache_state".into(), json!(input.cache));
    Ok(document)
}

pub(crate) fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
