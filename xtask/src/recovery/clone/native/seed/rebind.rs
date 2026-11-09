//! 来源重绑定的正式持锁阶段；只追加结果，不改写冻结来源或操作远端数据。
use crate::{
    Result,
    cli::{SeedSourceOperation, SeedSourceOptions},
    recovery::clone::native::{
        json as document,
        lifecycle::{
            stage::{self, StageRequest, StageRunner},
            state,
        },
        resources::common,
    },
};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[path = "rebind/authority.rs"]
mod authority;
#[path = "rebind/diagnostic.rs"]
mod diagnostic;
#[path = "rebind/observation.rs"]
mod observation;
#[path = "rebind/quiet.rs"]
mod quiet;
pub(crate) use authority::Controller;
pub(crate) use diagnostic::Phase;
pub(crate) use diagnostic::diagnostic;

pub(crate) fn run(backend: &Path, options: &SeedSourceOptions) -> Result<Value> {
    if options.operation != SeedSourceOperation::Rebind || !options.write {
        return Err("来源重绑定只接受正式 source-rebind --write".into());
    }
    let path = options
        .request
        .as_deref()
        .ok_or("来源重绑定必须提供 successor 请求")?;
    let path = common::common_file(backend, path, "重绑定 successor 请求")?;
    let runner = Runner {
        backend: backend.to_owned(),
        request: common::binding(&path)?,
    };
    stage::run_named(
        backend,
        &options.run_dir,
        "seed-runtime",
        "source-rebind",
        &runner,
    )
}

struct Runner {
    backend: PathBuf,
    request: Value,
}

impl StageRunner for Runner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        let result = (|| {
            authority::stage(request, false)?;
            if request.backend != self.backend {
                return Err("来源重绑定后端与正式入口不同".into());
            }
            common::local_directory(&self.backend, request.run_dir, "重绑定运行目录")?;
            common::bound_file(&self.backend, &self.request, "重绑定 successor 请求")?;
            let manifest = document::read(&request.run_dir.join("manifest.json"))?;
            if manifest["copy_stage"] != "source_to_seed" {
                return Err("来源重绑定只能继承完整 source_to_seed 运行".into());
            }
            state::load(request.run_dir, true)?;
            Ok(())
        })();
        classify(Phase::Preflight, result)
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        let result = (|| {
            if request.backend != self.backend {
                return Err("来源重绑定后端与正式入口不同".into());
            }
            let controller = classify(Phase::Authority, Controller::load(request, &self.request))?;
            let result = controller.checked(|| {
                classify(Phase::Quiet, quiet::require(&controller))?;
                observation::collect(&controller)
            });
            finish(result, classify(Phase::Checkpoint, controller.checkpoint()))
        })();
        classify(Phase::Authority, result)
    }
}

pub(crate) fn classify<T>(phase: Phase, result: Result<T>) -> Result<T> {
    result.map_err(|error| diagnostic::failure(phase, error, None))
}

pub(super) fn finish<T>(result: Result<T>, checked: Result<()>) -> Result<T> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), checked) => Err(diagnostic::failure(
            Phase::Comparison,
            primary,
            checked.err(),
        )),
        (Ok(_), Err(primary)) => Err(diagnostic::failure(Phase::Checkpoint, primary, None)),
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/seed/rebind.rs"]
mod tests;
