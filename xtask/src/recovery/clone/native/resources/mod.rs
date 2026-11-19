use std::path::Path;

use serde_json::Value;

use super::lifecycle::stage::{self, StageRequest, StageRunner};
use crate::{
    Result,
    cli::{CloneCommand, StorageOperation, StorageOptions},
};

#[cfg(windows)]
use crate::cli::CloneSide;

pub(crate) mod common;
pub(super) mod origin;
mod producers;
pub(crate) mod storage;
#[cfg(windows)]
mod storage_source_windows;
#[cfg(windows)]
pub(crate) use storage_source_windows::run_hidden_if_requested;

pub(crate) fn run(command: &CloneCommand, backend: &Path) -> Result<Value> {
    match command {
        CloneCommand::Storage(options) if options.operation == StorageOperation::Status => {
            storage::status(backend, &options.run_dir, options.side)
        }
        #[cfg(windows)]
        CloneCommand::Storage(options)
            if options.side == CloneSide::Source
                && options.operation == StorageOperation::Restart =>
        {
            storage_source_windows::run(backend, options)
        }
        CloneCommand::Storage(options) => run_storage_stage(backend, options),
        _ => Err("clone 资源模块不处理该命令".into()),
    }
}

fn run_storage_stage(backend: &Path, options: &StorageOptions) -> Result<Value> {
    let runner = ResourceRunner::Storage(options.clone());
    let stage = format!("storage-{}", options.side.as_str());
    stage::run_named(
        backend,
        &options.run_dir,
        &stage,
        options.operation.as_str(),
        &runner,
    )
}

enum ResourceRunner {
    Storage(StorageOptions),
}

impl StageRunner for ResourceRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        match self {
            Self::Storage(options) => {
                ensure_stage(request, &format!("storage-{}", options.side.as_str()))?;
                storage::preflight(request.backend, request.run_dir, options)
            }
        }
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        let attempt = request.attempt.ok_or("资源执行缺少阶段序号")?;
        match self {
            Self::Storage(options) => {
                storage::execute(request.backend, request.run_dir, attempt, options)
            }
        }
    }
}

fn ensure_stage(request: &StageRequest<'_>, expected: &str) -> Result<()> {
    if request.stage != expected {
        return Err(format!("资源阶段不匹配：期望 {expected}，实际 {}", request.stage).into());
    }
    Ok(())
}
