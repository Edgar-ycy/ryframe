//! 旧复制运行的 recover 只回收已退出控制器的原外层锁。
use super::{
    lock::ProcessGuard,
    manifest,
    runtime_lock::Original,
    runtime_selection,
    stage::{self, StageRequest, StageRunner},
    state,
};
use crate::{
    Result,
    cli::{CloneRuntimeOptions, CloneSide},
    recovery::clone::native::{
        cache::handoff, json as document, resources::common, target_verify::guard::Inputs,
    },
};
use serde_json::Value;
use std::path::Path;

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    options: &CloneRuntimeOptions,
) -> Result<Value> {
    let stage = match options.side {
        CloneSide::Source => "runtime-source",
        CloneSide::Target => "runtime-target",
    };
    let mut result = stage::run_named(
        backend,
        &options.run_dir,
        stage,
        "recover",
        &Runner(options),
    )?;
    result
        .as_object_mut()
        .ok_or("运行回收摘要无效")?
        .remove("result");
    Ok(result)
}

struct Runner<'a>(&'a CloneRuntimeOptions);

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        let manifest =
            manifest::registered(request.backend, &request.run_dir.join("manifest.json"))?;
        common::environment(
            request.backend,
            &manifest.value[runtime_selection::fields(self.0.side).0],
        )?;
        Ok(())
    }

    fn guarded_preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if state::running_attempt(&state::load(request.run_dir, false)?)?.is_some() {
            return Err("存在未收尾阶段；请使用原控制器收据显式执行 clone recover".into());
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        if handoff::selected(request.backend, self.0)? {
            return Err("已登记 post-copy 目标必须由原生完整树回收入口处理".into());
        }
        let manifest_path = request.run_dir.join("manifest.json");
        let manifest = manifest::registered(request.backend, &manifest_path)?;
        let (environment, selection) = runtime_selection::fields(self.0.side);
        let inputs = Inputs::open([
            manifest_path.clone(),
            common::bound_file(request.backend, &manifest.value[environment], "原环境")?,
            common::bound_file(request.backend, &manifest.value[selection], "原运行选择")?,
        ])?;
        if manifest::registered(request.backend, &manifest_path)?.digest != manifest.digest {
            return Err("取得回收输入保护时，原运行清单发生变化".into());
        }
        runtime_selection::environment(request.backend, &manifest.value, self.0.side)?;
        let selected = runtime_selection::selected(request.backend, &manifest.value, self.0.side)?;
        let runtime = runtime_selection::directory(Path::new(document::text(
            &selected["runtime_dir"],
            "已登记运行目录",
        )?))?;
        let guard = ProcessGuard::acquire_named(&runtime, "runtime-control.guard")?;
        let original = Original::load(&runtime)?;
        let actual = original.check()?;
        inputs.check()?;
        guard.check()?;
        let value = original.publish(&actual)?;
        inputs.check()?;
        guard.check()?;
        original.remove(&actual)?;
        guard.check()?;
        Ok(value)
    }
}
