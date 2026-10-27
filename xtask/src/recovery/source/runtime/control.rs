//! generation 停止与恢复的正式原生控制器；统一阶段入口持锁执行，不借用 Python 父 owner。
use crate::{
    Result,
    cli::{SeedSourceOperation, SeedSourceOptions},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::clone::native::{
        export::transport::SystemTransport,
        lifecycle::{
            manifest,
            stage::{self, StageRequest, StageRunner},
            state,
        },
        resources::common,
    },
};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[path = "control/configuration.rs"]
mod configuration;
#[path = "control/existing.rs"]
mod existing;
#[path = "control/frame.rs"]
mod frame;
#[path = "control/lineage.rs"]
mod lineage;
#[path = "control/observation.rs"]
mod observation;
#[path = "control/prelaunch.rs"]
mod prelaunch;
#[path = "control/publication.rs"]
mod publication;
#[path = "control/recover.rs"]
mod recover;
#[path = "control/start.rs"]
mod start;
#[path = "control/status.rs"]
mod status;
#[path = "control/stop.rs"]
mod stop;
#[path = "control/trees.rs"]
mod trees;
#[path = "control/window.rs"]
mod window;

pub(crate) fn run(backend: &Path, options: &SeedSourceOptions) -> Result<Value> {
    if options.operation == SeedSourceOperation::GenerationStatus {
        if options.write || options.request.is_some() {
            return Err("source-generation-status 只读且不接受替代请求".into());
        }
        let run = run_directory(backend, &options.run_dir)?;
        return status::run(backend, &run);
    }
    if !matches!(
        options.operation,
        SeedSourceOperation::GenerationStop
            | SeedSourceOperation::GenerationRecover
            | SeedSourceOperation::GenerationStart
    ) || !options.write
    {
        return Err("原生源控制只接受正式 source-generation-start/stop/recover --write".into());
    }
    let receipt = options
        .request
        .as_ref()
        .ok_or("源控制必须显式绑定停止收据或恢复请求")?;
    validate_local_test_path(receipt, backend, LocalTestPathKind::ExistingFile)?;
    let runner = GenerationRunner {
        backend: backend.to_owned(),
        receipt: receipt.clone(),
        mode: options.operation.as_str(),
    };
    let run = run_directory(backend, &options.run_dir)?;
    stage::run_named(
        backend,
        &run,
        "seed-runtime",
        options.operation.as_str(),
        &runner,
    )
}

fn run_directory(backend: &Path, path: &Path) -> Result<PathBuf> {
    validate_local_test_path(path, backend, LocalTestPathKind::ExistingDirectory)?;
    // 新控制记录与角色引擎使用同一真实目录拼法；不改写任何既有描述符。
    crate::recovery::clone::native::target_verify::input::request_configuration::canonical_path(
        path,
    )
}

struct GenerationRunner {
    backend: PathBuf,
    receipt: PathBuf,
    mode: &'static str,
}

impl StageRunner for GenerationRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.backend != self.backend || request.attempt.is_some() || request.mode != self.mode
        {
            return Err("源控制预检的后端、模式或 attempt 无效".into());
        }
        frame::stage(request)?;
        let run = request.run_dir;
        let initial = manifest::load(&self.backend, &run.join("manifest.json"))?;
        if initial.value["copy_stage"] != "source_to_seed" {
            return Err("源控制只能继承 source_to_seed 运行清单".into());
        }
        let binding = common::binding(&run.join("state.json"))?;
        let saved = state::load(run, true)?;
        if state::running_attempt(&saved)?.is_some() {
            return Err("源控制预检不能接管正在运行的阶段，保留控制器现场".into());
        }
        if self.mode == "source-generation-start" {
            start::preflight(&self.backend, run, state::attempts(&saved)?, &self.receipt)?;
        } else if self.mode == "source-generation-stop" {
            preflight_stop(&self.backend, run, &saved)?;
        } else {
            recover::preflight(&self.backend, run, &saved, &self.receipt)?;
        }
        let after = manifest::load(&self.backend, &run.join("manifest.json"))?;
        if after.value != initial.value
            || after.digest != initial.digest
            || state::load(run, true)? != saved
            || common::binding(&run.join("state.json"))? != binding
        {
            return Err("源控制预检期间完整清单或阶段历史变化".into());
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        if request.backend != self.backend || request.mode != self.mode {
            return Err("源控制执行根或模式发生变化".into());
        }
        frame::stage(request)?;
        let frame = frame::Frame::load(request)?;
        let result = if self.mode == "source-generation-start" {
            start::run(&frame, &self.receipt, &SystemTransport)
        } else if self.mode == "source-generation-stop" {
            stop::run(&frame, &self.receipt, &SystemTransport)
        } else {
            recover::run(&frame, &self.receipt, &SystemTransport)
        };
        let checked = frame.checkpoint();
        match (result, checked) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(checked)) => Err(format!(
                "源控制失败：{error}；控制区间退出核验也失败：{checked}；保留现场，禁止重放"
            )
            .into()),
        }
    }
}

fn preflight_stop(backend: &Path, run: &Path, saved: &Value) -> Result<()> {
    let attempts = state::attempts(saved)?;
    let starts = super::running::history::starts(backend, run, attempts)?;
    let [start] = starts.as_slice() else {
        return Err("源停止必须继承唯一已登记 start".into());
    };
    let number = start["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or("原 START 序号无效")?;
    if start["status"] != "passed"
        || attempts.iter().any(|row| {
            row["number"].as_u64().is_some_and(|n| n > number)
                && row["stage"] == "seed-runtime"
                && matches!(
                    row["mode"].as_str(),
                    Some("source-generation-stop" | "source-generation-recover")
                )
        })
    {
        return Err("源代次 stop 只能执行一次；失败只能精确回收，禁止重放".into());
    }
    Ok(())
}
