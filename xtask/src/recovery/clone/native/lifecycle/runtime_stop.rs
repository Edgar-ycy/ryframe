//! 旧运行按原角色收据停止单个产品进程，保留 v1 停止历史。
use super::{
    identity,
    lock::ProcessGuard,
    manifest, runtime_history,
    runtime_lock::Active,
    runtime_selection,
    runtime_status::{Context, role_identity, role_receipt},
    stage::{self, StageRequest, StageRunner},
    state,
};
use crate::{
    Result,
    cli::{CloneRole, CloneRuntimeOptions, CloneSide},
    recovery::{
        clone::native::{
            cache::handoff,
            json as document,
            resources::common,
            target_verify::{authority, guard::Inputs},
        },
        fixture_control::services::lifecycle::stop_recovery_process,
    },
};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    options: &CloneRuntimeOptions,
) -> Result<Value> {
    let stage = match options.side {
        CloneSide::Source => "runtime-source",
        CloneSide::Target => "runtime-target",
    };
    let mut summary = stage::run_named(backend, &options.run_dir, stage, "stop", &Runner(options))?;
    summary
        .as_object_mut()
        .ok_or("停止阶段摘要无效")?
        .remove("result");
    Ok(summary)
}

struct Runner<'a>(&'a CloneRuntimeOptions);

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        runtime_selection::validate_roles(&self.0.roles)?;
        let saved = manifest::registered(request.backend, &request.run_dir.join("manifest.json"))?;
        check_environment(request.backend, &saved.value, self.0.side)
    }

    fn guarded_preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if state::running_attempt(&state::load(request.run_dir, false)?)?.is_some() {
            return Err("存在未收尾阶段；请使用原控制器收据显式执行 clone recover".into());
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        if handoff::selected(request.backend, self.0)? {
            return Err("已登记 post-copy 目标必须由原生完整树停止入口处理".into());
        }
        let manifest_path = request.run_dir.join("manifest.json");
        let saved = manifest::registered(request.backend, &manifest_path)?;
        let (environment, selection) = runtime_selection::fields(self.0.side);
        let inputs = Inputs::open([
            manifest_path.clone(),
            common::bound_file(request.backend, &saved.value[environment], "原环境")?,
            common::bound_file(request.backend, &saved.value[selection], "原运行选择")?,
        ])?;
        if manifest::registered(request.backend, &manifest_path)?.digest != saved.digest {
            return Err("取得停止输入保护时，原运行清单发生变化".into());
        }
        check_environment(request.backend, &saved.value, self.0.side)?;
        let selected = runtime_selection::selected(request.backend, &saved.value, self.0.side)?;
        let directory = runtime_selection::directory(Path::new(document::text(
            &selected["runtime_dir"],
            "已登记运行目录",
        )?))?;
        let guard = ProcessGuard::acquire_named(&directory, "runtime-control.guard")?;
        let active = Active::claim(&directory)?;
        let result = (|| {
            let context = Context::load(request.backend, &selected)?;
            if context.directory != directory {
                return Err("停止运行目录在取得外层控制锁后变化".into());
            }
            stop_roles(
                request.backend,
                &context,
                &self.0.roles,
                &guard,
                &active,
                &inputs,
            )
        })();
        guard.check()?;
        match (result, active.release()) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(_), Err(_)) => Err("运行停止失败且原外层锁未能精确释放，保留现场".into()),
        }
    }
}

fn check_environment(backend: &Path, manifest: &Value, side: CloneSide) -> Result<()> {
    let environment = runtime_selection::environment(backend, manifest, side)?;
    if !environment.iter().any(|(name, value)| {
        (name == "APP_ENV" || cfg!(windows) && name.eq_ignore_ascii_case("APP_ENV"))
            && value == "test"
    }) {
        return Err("运行停止要求原登记环境的 APP_ENV=test".into());
    }
    Ok(())
}

struct Process {
    role: CloneRole,
    path: PathBuf,
    receipt: Option<Value>,
    live: Option<Value>,
}

impl Process {
    fn observe(context: &Context, role: CloneRole) -> Result<Self> {
        let path = context.directory.join(format!("{}.json", role.as_str()));
        let receipt = role_receipt(&path)?;
        let mut live = None;
        if let Some(receipt) = &receipt {
            let expected = role_identity(receipt, &context.binding, role)?;
            let pid = u32::try_from(expected["pid"].as_u64().ok_or("登记角色 PID 无效")?)?;
            live = identity::process(pid)?;
            if live.as_ref().is_some_and(|actual| actual != &expected) {
                return Err("登记角色 PID 已复用或创建身份变化".into());
            }
        }
        Ok(Self {
            role,
            path,
            receipt,
            live,
        })
    }

    fn check(&self) -> Result<()> {
        if role_receipt(&self.path)? != self.receipt {
            return Err("角色收据在停止期间变化，保留现场".into());
        }
        Ok(())
    }
}

fn stop_roles(
    backend: &Path,
    context: &Context,
    roles: &[CloneRole],
    guard: &ProcessGuard,
    active: &Active,
    inputs: &Inputs,
) -> Result<Value> {
    // 任何角色身份未知时，不先停止另一个角色。
    let roles = roles
        .iter()
        .map(|role| Process::observe(context, *role))
        .collect::<Result<Vec<_>>>()?;
    let mut processes = Map::new();
    for process in roles.iter().rev() {
        guard.check()?;
        active.check()?;
        inputs.check()?;
        context.check(backend)?;
        for original in &roles {
            original.check()?;
        }
        if let Some(identity) = &process.live {
            stop_recovery_process(identity)?;
            active.check()?;
            process.check()?;
            runtime_history::stopped(
                &context.directory,
                &context.binding.scope,
                process.role.as_str(),
                identity,
            )?;
        }
        authority::require_closed(context.endpoint(process.role).0)?;
        processes.insert(
            process.role.as_str().into(),
            json!({"state":"stopped","identity":null,"ready":false}),
        );
    }
    inputs.check()?;
    context.check(backend)?;
    let history = context.directory.join(runtime_history::FILE);
    Ok(
        json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"stop",
        "scope_id":context.binding.scope,"runtime_directory":context.directory,"processes":processes,
        "producer_history":history.is_file().then_some(history)}),
    )
}
