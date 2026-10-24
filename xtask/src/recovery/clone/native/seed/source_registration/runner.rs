//! source-register 的正式阶段执行叶子；父入口负责通过 run_named 调度。
use super::{materialize, quiet};
use crate::{
    Result,
    recovery::clone::native::{
        bridge::scope::Scope,
        input, json as document,
        lifecycle::{
            identity,
            lock::ProcessGuard,
            manifest,
            stage::{StageRequest, StageRunner},
            state,
        },
        resources::common,
        seed::{Registration, history},
        target_verify::guard::Generation,
    },
};
use serde_json::{Value, json};
use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

pub(crate) struct SourceRegisterRunner {
    backend: PathBuf,
}
impl SourceRegisterRunner {
    pub(crate) fn new(backend: &Path) -> Self {
        Self {
            backend: backend.to_owned(),
        }
    }
    fn request(&self, request: &StageRequest<'_>, executing: bool) -> Result<Option<u64>> {
        if request.backend != self.backend
            || request.stage != "seed-runtime"
            || request.mode != "source-register"
            || if executing {
                request.attempt.is_none_or(|n| n == 0)
            } else {
                request.attempt.is_some()
            }
        {
            return Err("来源登记阶段、模式、后端或 attempt 不符合统一阶段协议".into());
        }
        Ok(request.attempt)
    }
    fn inspect_preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        self.request(request, false)?;
        let before = manifest::load(&self.backend, &request.run_dir.join("manifest.json"))?;
        source_stage(&before.value)?;
        let saved = state::load(request.run_dir, true)?;
        require_state(&saved, None)?;
        let registration = Registration::load(&self.backend, request.run_dir, false)?;
        history::validate(&self.backend, request.run_dir, &registration.value)?;
        registration.assert_unchanged(&self.backend, false)?;
        let after = manifest::load(&self.backend, &request.run_dir.join("manifest.json"))?;
        if before.value != after.value
            || before.digest != after.digest
            || state::load(request.run_dir, true)? != saved
        {
            return Err("来源登记预检期间完整清单或阶段记录变化".into());
        }
        Ok(())
    }
    fn execute_bound(&self, request: &StageRequest<'_>) -> Result<Value> {
        let number = self
            .request(request, true)?
            .ok_or("来源登记缺少实际 attempt")?;
        let frame = Frame::load(&self.backend, request.run_dir, number)?;
        let paths = Paths::load(&frame)?;
        absent_ledger_lock(&paths.ledger)?;
        let generation = Generation::acquire(&self.backend, &paths.target)?;
        let result = (|| {
            let ledger = ProcessGuard::acquire_named(&paths.ledger, "copy-ledger.guard")?;
            let result = (|| {
                absent_ledger_lock(&paths.ledger)?;
                let bindings = document::array(&frame.manifest.value["build_bridges"], "构建桥接")?;
                let scope = Scope::load(&self.backend, bindings, &[])?;
                let tool = coordinator_tool()?;
                let window = Window {
                    frame: &frame,
                    paths: &paths,
                    generation: &generation,
                    ledger: &ledger,
                    scope: &scope,
                };
                window.check()?;
                let tool_path = Path::new(document::text(&tool["path"], "协调器工具")?);
                let mut observe = |expected: &Value| {
                    window.check()?;
                    let result =
                        quiet::require_stopped(&self.backend, request.run_dir, expected, tool_path);
                    preserve(result, window.check(), "quiet 后置核验")
                };
                let result = materialize::materialize(
                    &self.backend,
                    request.run_dir,
                    number,
                    &tool,
                    &scope,
                    &generation,
                    &mut observe,
                );
                preserve(result, window.check(), "发布后置核验")
            })();
            let checked = preserve(generation.check(), ledger.check(), "复制账本 guard");
            let checked = preserve(checked, paths.check(&self.backend), "目标与账本路径");
            preserve(
                result,
                preserve(checked, frame.check(), "控制器后置核验"),
                "执行窗口后置核验",
            )
        })();
        // 分开释放与主结果，避免 Generation 的复合错误格式化丢失原始错误对象。
        let released = generation.finish(Ok(()));
        preserve(result, released, "目标 Generation 释放")
    }
}
impl StageRunner for SourceRegisterRunner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        self.inspect_preflight(request)
            .map_err(|error| report("预检", error, None))
    }
    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.execute_bound(request)
            .map_err(|error| report("执行", error, None))
    }
}

struct Frame<'a> {
    backend: &'a Path,
    run: &'a Path,
    number: u64,
    manifest: manifest::Manifest,
    saved: Value,
    state_binding: Value,
    controller: Value,
    owner: Value,
    owner_binding: Value,
}
impl<'a> Frame<'a> {
    fn load(backend: &'a Path, run: &'a Path, number: u64) -> Result<Self> {
        let manifest = manifest::load(backend, &run.join("manifest.json"))?;
        source_stage(&manifest.value)?;
        let state_binding = common::binding(&run.join("state.json"))?;
        let saved = state::load(run, true)?;
        let attempt = require_state(&saved, Some(number))?.ok_or("来源登记缺少运行阶段")?;
        let (controller, owner) = state::controller_record(run, number, attempt)?;
        authorized_owner(&owner, run, &manifest.digest.sha256, &identity::current()?)?;
        let owner_binding = common::binding(&run.join("run.lock/owner.json"))?;
        if common::read_bound(backend, &owner_binding, "统一运行 owner")? != owner {
            return Err("来源登记控制器不是当前统一运行锁持有人".into());
        }
        let frame = Self {
            backend,
            run,
            number,
            manifest,
            saved,
            state_binding,
            controller,
            owner,
            owner_binding,
        };
        frame.check()?;
        Ok(frame)
    }
    fn check(&self) -> Result<()> {
        let manifest = manifest::load(self.backend, &self.run.join("manifest.json"))?;
        let saved = state::load(self.run, true)?;
        let attempt = require_state(&saved, Some(self.number))?.ok_or("来源登记运行阶段缺失")?;
        let (controller, owner) = state::controller_record(self.run, self.number, attempt)?;
        authorized_owner(
            &owner,
            self.run,
            &manifest.digest.sha256,
            &identity::current()?,
        )?;
        if manifest.value != self.manifest.value
            || manifest.digest != self.manifest.digest
            || saved != self.saved
            || common::binding(&self.run.join("state.json"))? != self.state_binding
            || controller != self.controller
            || owner != self.owner
            || common::binding(&self.run.join("run.lock/owner.json"))? != self.owner_binding
            || common::read_bound(self.backend, &self.owner_binding, "统一运行 owner")? != owner
        {
            return Err("来源登记期间完整清单、阶段、控制器或持锁身份变化".into());
        }
        Ok(())
    }
}
struct Paths {
    target: PathBuf,
    ledger: PathBuf,
}
impl Paths {
    fn load(frame: &Frame<'_>) -> Result<Self> {
        let initialized = common::bound_file(
            frame.backend,
            &frame.manifest.value["initialized"],
            "目标初始化完整绑定",
        )?;
        let target = common::local_directory(
            frame.backend,
            initialized.parent().ok_or("目标初始化缺少父目录")?,
            "复制目标",
        )?;
        let copy = common::local_directory(
            frame.backend,
            Path::new(document::text(
                &frame.manifest.value["copy_directory"],
                "复制目录",
            )?),
            "复制目录",
        )?;
        let ledger = common::local_directory(frame.backend, &copy.join("ledger"), "复制账本")?;
        Ok(Self { target, ledger })
    }
    fn check(&self, backend: &Path) -> Result<()> {
        common::local_directory(backend, &self.target, "复制目标")?;
        common::local_directory(backend, &self.ledger, "复制账本")?;
        absent_ledger_lock(&self.ledger)
    }
}
struct Window<'a, 'b> {
    frame: &'a Frame<'b>,
    paths: &'a Paths,
    generation: &'a Generation,
    ledger: &'a ProcessGuard,
    scope: &'a Scope,
}
impl Window<'_, '_> {
    fn check(&self) -> Result<()> {
        let result = preserve(
            self.generation.check(),
            self.ledger.check(),
            "复制账本 guard",
        );
        let result = preserve(
            result,
            self.paths.check(self.frame.backend),
            "目标及账本路径",
        );
        let result = preserve(result, self.frame.check(), "完整控制器前像");
        preserve(result, self.scope.assert_unchanged(), "来源 Scope")
    }
}

fn source_stage(manifest: &Value) -> Result<()> {
    if manifest["copy_stage"] != "source_to_seed" {
        return Err("只有 source_to_seed 运行可登记来源".into());
    }
    Ok(())
}
fn require_state(saved: &Value, current: Option<u64>) -> Result<Option<&Value>> {
    let attempts = state::attempts(saved)?;
    if attempts.iter().any(|row| {
        row["stage"] == "seed-runtime"
            && row["mode"] == "source-register"
            && row["status"] == "passed"
    }) {
        return Err("来源登记已发布成功，禁止再次发布或自动重放".into());
    }
    let running = state::running_attempt(saved)?;
    match (current, running) {
        (None, None) => Ok(None),
        (Some(number), Some(attempt))
            if number > 0
                && attempt["number"].as_u64() == Some(number)
                && attempt["stage"] == "seed-runtime"
                && attempt["mode"] == "source-register"
                && attempts
                    .iter()
                    .filter(|row| row["number"].as_u64() == Some(number))
                    .count()
                    == 1 =>
        {
            Ok(Some(attempt))
        }
        _ => Err("来源登记必须在唯一对应的运行阶段内执行，预检不得已有运行阶段".into()),
    }
}
fn authorized_owner(owner: &Value, run: &Path, manifest_sha: &str, current: &Value) -> Result<()> {
    identity::validate(owner, run)?;
    input::digest_shape(&owner["manifest_sha256"])?;
    if owner["manifest_sha256"] != manifest_sha || owner["identity"] != *current {
        return Err("来源登记控制器必须绑定完整清单及实际当前原生进程身份".into());
    }
    Ok(())
}
fn absent_ledger_lock(ledger: &Path) -> Result<()> {
    match fs::symlink_metadata(ledger.join("clone.lock")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err("复制账本 clone.lock 已存在；保留现场，禁止抢锁或重放".into()),
    }
}
fn coordinator_tool() -> Result<Value> {
    let binding = crate::check::python_suite::native_tool_binding()?;
    let path = document::text(&binding["path"], "当前原生工具路径")?;
    if !Path::new(path).is_absolute() {
        return Err("协调器工具必须是当前显式绝对路径".into());
    }
    input::digest_shape(&binding["sha256"])?;
    Ok(json!({"path":path,"sha256":binding["sha256"]}))
}

// 两侧原始诊断保留在对象中，对外格式化只公开类别和安全 OS 数字。
struct Failure {
    context: &'static str,
    primary: Box<dyn Error>,
    secondary: Option<Box<dyn Error>>,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "来源登记{}失败：{}",
            self.context,
            safe_reason(self.primary.as_ref())
        )?;
        if let Some(error) = &self.secondary {
            write!(f, "；后置操作同时失败：{}", safe_reason(error.as_ref()))?;
        }
        write!(
            f,
            "；原始诊断保留在错误链中。请核对控制器、绑定文件、来源和锁现场，修复后显式重试；禁止自动抢锁或重放。"
        )
    }
}
impl fmt::Debug for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl Error for Failure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.primary.as_ref())
    }
}
fn safe_reason(error: &(dyn Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<Failure>() {
        return error.to_string();
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        format!(
            "I/O {:?}，OS错误码 {:?}",
            error.kind(),
            error.raw_os_error()
        )
    } else {
        "证据、身份、锁或来源校验未通过".into()
    }
}
fn report(
    context: &'static str,
    primary: Box<dyn Error>,
    secondary: Option<Box<dyn Error>>,
) -> Box<dyn Error> {
    if secondary.is_none() && primary.is::<Failure>() {
        return primary;
    }
    Box::new(Failure {
        context,
        primary,
        secondary,
    })
}
fn preserve<T>(result: Result<T>, checked: Result<()>, context: &'static str) -> Result<T> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(primary), Err(secondary)) => Err(report(context, primary, Some(secondary))),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
    }
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/runner.rs"]
mod tests;
