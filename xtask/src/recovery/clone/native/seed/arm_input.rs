//! 在同一源控制器与目标存储运行锁内发布 ARM 输入。
use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::SeedRuntimeOptions,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{
            cache::{handoff::locks::Storage, runner::bridges_for_source},
            json as document,
            lifecycle::{
                identity, lock, manifest,
                stage::{self, StageRequest, StageRunner},
                state,
            },
            post_process::{self, files::combine},
            resources::common,
            target_verify::guard::Inputs,
        },
        fixture_control::successor::arm_target::{self, Target},
        source::evidence::comparison::arm::{self, Source},
    },
};

#[path = "arm_input/history.rs"]
pub(in crate::recovery) mod history;
#[path = "arm_input/model.rs"]
mod model;

const MODE: &str = "arm-input";

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    options: &SeedRuntimeOptions,
) -> Result<Value> {
    let path = options
        .request
        .as_deref()
        .ok_or("arm-input 需要明确 --request")?;
    if options.producer_binding.is_some() {
        return Err("arm-input 不接受生产者收据参数".into());
    }
    let input = common::binding(&common::common_file(backend, path, "arm-input 请求")?)?;
    let value = stage::run_named(
        backend,
        &options.run_dir,
        "seed-runtime",
        MODE,
        &Runner { input },
    )?;
    Ok(
        json!({"status":value["status"],"stage":value["stage"],"mode":value["mode"],
        "attempt":value["attempt"],"restore_qualified":value["restore_qualified"]}),
    )
}

struct Runner {
    input: Value,
}

impl StageRunner for Runner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.stage != "seed-runtime" || request.mode != MODE {
            return Err("arm-input 必须在同一正式 seed 阶段执行".into());
        }
        let value = common::read_bound(request.backend, &self.input, "arm-input 请求")?;
        model::request(&value)?;
        copy_directory(request.backend, &value)?;
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.preflight(request)?;
        let frame = Frame::load(request, &self.input)?;
        post_process::require_quiet(frame.backend, frame.run, Some(frame.number))?;
        let initial = frame.observe(false)?;
        let manifest = initial.manifest(&frame.request);
        let storage = Storage::acquire(frame.backend, frame.run, &manifest)?;
        let published = frame.publish(&initial, &manifest, &storage);
        storage.finish(published)
    }
}

struct Frame<'a> {
    backend: &'a Path,
    run: &'a Path,
    number: u64,
    input: &'a Value,
    request: Value,
    saved: Value,
    protection: Inputs,
}

impl<'a> Frame<'a> {
    fn load(stage: &StageRequest<'a>, input: &'a Value) -> Result<Self> {
        let number = stage
            .attempt
            .filter(|n| *n > 0)
            .ok_or("arm-input 缺少当前序号")?;
        let backend = stage.backend;
        let run = stage.run_dir;
        let path = common::bound_file(backend, input, "arm-input 请求")?;
        let protection = Inputs::open([path, run.join("manifest.json"), run.join("state.json")])?;
        let saved = state::load(run, true)?;
        let request = common::read_bound(backend, input, "arm-input 请求")?;
        let frame = Self {
            backend,
            run,
            number,
            input,
            request,
            saved,
            protection,
        };
        frame.check()?;
        Ok(frame)
    }

    fn check(&self) -> Result<()> {
        self.protection.check()?;
        common::bound_file(self.backend, self.input, "arm-input 请求")?;
        if state::load(self.run, true)? != self.saved {
            return Err("arm-input 阶段账本在核验期间变化".into());
        }
        check_owner(self.run, &self.saved, self.number)
    }

    fn observe(&self, live: bool) -> Result<Observation> {
        self.check()?;
        let successor = model::request(&self.request)?;
        let copy = copy_directory(self.backend, &self.request)?;
        let authority = history::Active::new(self.run, self.number, self.input, &self.request);
        let source = arm::load(self.backend, &authority, successor, live)?;
        let target =
            arm_target::for_input(self.backend, &self.request, &source.value, successor, live)?;
        self.check()?;
        Ok(Observation {
            source,
            target,
            copy,
        })
    }

    fn publish(&self, initial: &Observation, manifest: &Value, storage: &Storage) -> Result<Value> {
        storage.check()?;
        let before = self.observe(true)?;
        if before != *initial || before.manifest(&self.request) != *manifest {
            return Err("arm-input 取得存储锁后目标或源证据变化".into());
        }
        let result = self.write(&before, manifest);
        let checked = (|| {
            let after = self.observe(true)?;
            if after != before || after.manifest(&self.request) != *manifest {
                return Err("arm-input 发布期间源、目标、存储或请求发生变化".into());
            }
            post_process::require_quiet(self.backend, self.run, Some(self.number))?;
            storage.check()?;
            self.check()
        })();
        let result = combine(result, checked)?;
        for field in ["request", "manifest"] {
            common::bound_file(self.backend, &result[field], "arm 最终输出")?;
        }
        Ok(result)
    }

    fn write(&self, before: &Observation, manifest: &Value) -> Result<Value> {
        manifest::validate(manifest, self.backend)?;
        bridges_for_source(self.backend, &before.source.value)?;
        let rows = state::attempts(&self.saved)?;
        let prior = history::published(self.backend, self.run, rows, &self.request)?;
        if prior.iter().any(|arm| arm.side == before.target.side) {
            return Err("同一 seed run 已发布该 arm 目标侧".into());
        }
        let path = history::pending(self.backend, self.run, rows, &prior, &self.request)?;
        let output = history::write_prefix(
            self.backend,
            self.run,
            self.number,
            path,
            &self.request,
            manifest,
        )?;
        Ok(model::result(
            &self.request,
            &before.source.value,
            &before.target.generation,
            &before.target.side,
            &common::binding(&output.join("request.json"))?,
            &common::binding(&output.join("manifest.json"))?,
        ))
    }
}

#[derive(PartialEq)]
struct Observation {
    source: Source,
    target: Target,
    copy: String,
}

impl Observation {
    fn manifest(&self, request: &Value) -> Value {
        model::manifest(
            request,
            &self.source.value,
            &self.target.generation,
            &self.copy,
        )
    }
}

fn copy_directory(backend: &Path, request: &Value) -> Result<String> {
    let path = Path::new(document::text(&request["copy_directory"], "arm 复制目录")?);
    let path = validate_local_test_path(path, backend, LocalTestPathKind::NewDirectory)?;
    manifest::path_text(&path, "arm 复制目录")
}

fn check_owner(run: &Path, saved: &Value, number: u64) -> Result<()> {
    let row = state::running_attempt(saved)?.ok_or("arm-input 缺少运行阶段")?;
    if row["number"] != number || row["stage"] != "seed-runtime" || row["mode"] != MODE {
        return Err("arm-input 不是当前登记阶段".into());
    }
    let (_, controller) = state::controller_record(run, number, row)?;
    let (_, owner) = lock::owner_observation(run)?.ok_or("arm-input 缺少当前运行锁")?;
    if owner != controller
        || owner["identity"] != identity::current()?
        || owner["manifest_sha256"] != state::manifest_binding(run)?["sha256"]
    {
        return Err("arm-input 必须由当前同一源运行控制器执行".into());
    }
    Ok(())
}
