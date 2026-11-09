//! seed 状态、停止和回收复用已发布角色合同与原阶段控制器。
use serde_json::Value;
use std::path::Path;

use super::pair_entry;
use crate::{
    Result,
    cli::{SeedRuntimeOperation, SeedRuntimeOptions},
    recovery::clone::native::{
        lifecycle::stage::{self, StageRequest, StageRunner},
        post_process::entry::summary,
        resources::common,
    },
};

pub(in crate::recovery::clone::native) fn selected(operation: SeedRuntimeOperation) -> bool {
    matches!(
        operation,
        SeedRuntimeOperation::Status | SeedRuntimeOperation::Stop | SeedRuntimeOperation::Recover
    )
}

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    options: &SeedRuntimeOptions,
) -> Result<Value> {
    if !selected(options.operation)
        || options.request.is_some()
        || options.producer_binding.is_some()
    {
        return Err("seed 角色观察和清理只使用已发布运行交接".into());
    }
    let run = common::local_directory(backend, &options.run_dir, "seed 固定运行目录")?;
    if options.operation == SeedRuntimeOperation::Status {
        return pair_entry::status(backend, &run);
    }
    let runner = Runner {
        mode: options.operation.as_str(),
    };
    let result = stage::run_named(backend, &run, "seed-runtime", runner.mode, &runner)?;
    Ok(summary(&result))
}

struct Runner {
    mode: &'static str,
}

impl StageRunner for Runner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.stage != "seed-runtime" || request.mode != self.mode {
            return Err("seed 清理不同于当前正式阶段".into());
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.preflight(request)?;
        let number = request.attempt.ok_or("seed 清理缺少正式阶段控制器")?;
        pair_entry::cleanup(request.backend, request.run_dir, number, self.mode)
    }
}
