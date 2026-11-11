//! 旧复制运行启动保留单进程收据和调用方角色顺序。
use super::{
    identity,
    lock::ProcessGuard,
    manifest, runtime_history,
    runtime_lock::Active,
    runtime_selection, runtime_start_target,
    runtime_status::{Context, role_identity, role_receipt},
    stage::{self, StageRequest, StageRunner},
    state,
};
use crate::{
    Result,
    cli::{CloneRole, CloneRuntimeOperation, CloneRuntimeOptions, CloneSide},
    recovery::clone::native::{
        bridge::scope::Scope,
        cache::{handoff, runner::inherited_bridges},
        json as document,
        resources::common,
        target_verify::{authority, guard::Inputs},
    },
};
use serde_json::{Map, Value, json};
use std::{cell::RefCell, collections::BTreeMap, path::Path};

#[path = "runtime_start_role.rs"]
mod role;

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    options: &CloneRuntimeOptions,
) -> Result<Value> {
    let stage = match options.side {
        CloneSide::Source => "runtime-source",
        CloneSide::Target => "runtime-target",
    };
    let mut summary = stage::run_named(
        backend,
        &options.run_dir,
        stage,
        "start",
        &Runner {
            options,
            scope: RefCell::new(None),
        },
    )?;
    summary
        .as_object_mut()
        .ok_or("启动阶段摘要无效")?
        .remove("result");
    Ok(summary)
}

struct Runner<'a> {
    options: &'a CloneRuntimeOptions,
    scope: RefCell<Option<Scope>>,
}

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        runtime_selection::validate_roles(&self.options.roles)?;
        let saved = manifest::load(request.backend, &request.run_dir.join("manifest.json"))?;
        environments(request.backend, &saved.value, self.options.side)?;
        *self.scope.borrow_mut() = Some(Scope::load(
            request.backend,
            document::array(&saved.value["build_bridges"], "旧运行构建桥接")?,
            &inherited_bridges(request.backend, &saved.value)?,
        )?);
        Ok(())
    }

    fn guarded_preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if state::running_attempt(&state::load(request.run_dir, true)?)?.is_some() {
            return Err("存在未收尾阶段；请使用原控制器收据显式执行 clone recover".into());
        }
        Ok(())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        if handoff::selected(request.backend, self.options)? {
            return Err("已登记 post-copy 目标必须由原生完整树启动入口处理".into());
        }
        let scope = self.scope.borrow();
        let scope = scope.as_ref().ok_or("旧运行启动缺少来源核验窗口")?;
        scope.assert_unchanged()?;
        let manifest_path = request.run_dir.join("manifest.json");
        let saved = manifest::load(request.backend, &manifest_path)?;
        let (_, selection) = runtime_selection::fields(self.options.side);
        let inputs = Inputs::open([
            manifest_path.clone(),
            common::bound_file(
                request.backend,
                &saved.value["source_environment"],
                "源环境",
            )?,
            common::bound_file(
                request.backend,
                &saved.value["target_environment"],
                "目标环境",
            )?,
            common::bound_file(request.backend, &saved.value[selection], "原运行选择")?,
        ])?;
        if manifest::load(request.backend, &manifest_path)?.digest != saved.digest {
            return Err("取得启动输入保护时，原运行清单发生变化".into());
        }
        let environment = environments(request.backend, &saved.value, self.options.side)?;
        let selected = match self.options.side {
            CloneSide::Source => {
                runtime_selection::selected(request.backend, &saved.value, CloneSide::Source)?
            }
            CloneSide::Target => runtime_start_target::verify(
                request.backend,
                request.run_dir,
                &saved.value,
                &self.options.roles,
            )?,
        };
        let context = Context::load(request.backend, &selected)?;
        let guard = ProcessGuard::acquire_named(&context.directory, "runtime-control.guard")?;
        let active = Active::claim(&context.directory, CloneRuntimeOperation::Start)?;
        let check_lock = || -> Result<()> {
            guard.check()?;
            active.check()
        };
        let check = || -> Result<()> {
            check_lock()?;
            inputs.check()?;
            context.check(request.backend)
        };
        let result = start_roles(
            request.backend,
            &context,
            &environment,
            &self.options.roles,
            check,
            check_lock,
            scope,
        );
        guard.check()?;
        match (result, active.release()) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(_), Err(_)) => Err("运行启动失败且原外层锁未能精确释放，保留现场".into()),
        }
    }
}

fn environments(
    backend: &Path,
    manifest: &Value,
    side: CloneSide,
) -> Result<BTreeMap<String, String>> {
    let source = runtime_selection::environment(backend, manifest, CloneSide::Source)?;
    let target = runtime_selection::environment(backend, manifest, CloneSide::Target)?;
    let selected = match side {
        CloneSide::Source => source,
        CloneSide::Target => target,
    };
    if !selected.iter().any(|(name, value)| {
        (name == "APP_ENV" || cfg!(windows) && name.eq_ignore_ascii_case("APP_ENV"))
            && value == "test"
    }) {
        return Err("运行启动要求原登记环境的 APP_ENV=test".into());
    }
    Ok(selected)
}

fn idle(context: &Context, role: CloneRole) -> Result<()> {
    let path = context.directory.join(format!("{}.json", role.as_str()));
    if let Some(receipt) = role_receipt(&path)? {
        let expected = role_identity(&receipt, &context.binding, role)?;
        let pid = u32::try_from(expected["pid"].as_u64().ok_or("登记角色 PID 无效")?)?;
        if let Some(actual) = identity::process(pid)? {
            if actual != expected {
                return Err("登记角色 PID 已复用或创建身份变化，禁止启动".into());
            }
            return Err("已登记进程仍在运行，拒绝重复启动".into());
        }
    }
    authority::require_closed(context.endpoint(role).0)
}

fn start_roles(
    backend: &Path,
    context: &Context,
    environment: &BTreeMap<String, String>,
    roles: &[CloneRole],
    check: impl Fn() -> Result<()>,
    check_lock: impl Fn() -> Result<()>,
    scope: &Scope,
) -> Result<Value> {
    check()?;
    for role in roles {
        idle(context, *role)?;
    }
    let mut started = Vec::new();
    let mut processes = Map::new();
    let result = (|| {
        for selected in roles {
            check()?;
            idle(context, *selected)?;
            let (process, value) =
                role::start(backend, context, environment, *selected, &check_lock)?;
            processes.insert(selected.as_str().into(), value);
            started.push(process);
        }
        check()?;
        scope.assert_unchanged()
    })();
    if let Err(error) = result {
        let mut failed_cleanup = false;
        for process in started.into_iter().rev() {
            failed_cleanup |= process.rollback(context, &check_lock).is_err();
        }
        if failed_cleanup {
            return Err("旧运行启动失败且本次启动回收未完全确认，保留原历史与日志".into());
        }
        return Err(error);
    }
    Ok(
        json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"start",
        "scope_id":context.binding.scope,"runtime_directory":context.directory,"processes":processes,
        "producer_history":context.directory.join(runtime_history::FILE)}),
    )
}
