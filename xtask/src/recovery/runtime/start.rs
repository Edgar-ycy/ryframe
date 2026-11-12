//! 在原登记锁内逐个启动正式恢复角色并发布完整运行代次。
use std::{path::Path, time::Duration};

use serde_json::{Value, json};

use super::{
    bind::ownership::{Owner, control_root},
    dataset_authority::{check, environment, inputs::Inputs, reads::Window},
    generation::ROLES,
    registration::backend_root,
    stop,
};
use crate::{Result, cli::RuntimeStartOptions};

#[path = "start/actor.rs"]
pub(crate) mod actor;
#[path = "start/launch.rs"]
mod launch;
#[path = "start/prepare.rs"]
mod prepare;
#[path = "start/readiness.rs"]
mod readiness;
#[path = "start/sources.rs"]
mod sources;
#[path = "start/state.rs"]
mod state;

use prepare::Prepared;
use state::State;

pub(super) fn run(root: &Path, options: &RuntimeStartOptions) -> Result<Value> {
    check(
        !options.timeout.is_zero() && options.timeout <= Duration::from_secs(180),
        "恢复运行就绪超时必须在 0 到 180 秒之间",
    )?;
    let backend = backend_root(root)?;
    let before = {
        let window = Window::direct()?;
        let mut inputs = Inputs::new(&backend);
        let prepared = Prepared::capture(&backend, options, &mut inputs);
        window.finish(inputs.finish(prepared))?
    };
    let control = control_root(&backend)?;
    Owner::with(&control, &before.bound.operation("start")?, |owner| {
        let mut inputs = Inputs::new(&backend);
        let locked = recapture(&backend, options, owner, &mut inputs)?;
        check(locked == before, "取得原运行控制权前启动来源发生变化")?;
        let mut state = State::begin(&backend, &locked, owner, &mut inputs)?;
        let mut attempts = Vec::new();
        let outcome = launch_roles(
            &backend,
            options,
            &locked,
            owner,
            &mut inputs,
            &mut state,
            &mut attempts,
        );
        let result = match outcome {
            Ok(launch) => Ok(json!({
                "format_version":1,"kind":"restore-runtime-lifecycle-result","operation":"start",
                "status":"running","generation":state.number(),"runtime_directory":state.directory(),
                "launch":launch,
            })),
            Err(error) => Err(cleanup(&backend, owner, &mut state, &mut attempts, error)),
        };
        for (_, attempt) in &mut attempts {
            attempt.release();
        }
        result
    })
}

fn recapture(
    backend: &Path,
    options: &RuntimeStartOptions,
    owner: &Owner,
    inputs: &mut Inputs,
) -> Result<Prepared> {
    let window = Window::direct()?;
    owner.check()?;
    let result = Prepared::capture(backend, options, inputs);
    window.finish_runtime_owner(owner, inputs.finish(result))
}

fn launch_roles(
    backend: &Path,
    options: &RuntimeStartOptions,
    prepared: &Prepared,
    owner: &Owner,
    inputs: &mut Inputs,
    state: &mut State,
    attempts: &mut Vec<(&'static str, launch::Attempt)>,
) -> Result<Value> {
    owner.check()?;
    environment::materialize(inputs, &prepared.request, state.directory())?;
    owner.check()?;
    for role in ROLES {
        inputs.finish(Ok(()))?;
        let attempt = launch::Attempt::new(backend, state, role)?;
        attempts.push((role, attempt));
        let tree = attempts
            .last_mut()
            .ok_or("恢复启动缺少当前监督尝试")?
            .1
            .start(state, owner, &std::env::current_exe()?)?;
        state.update(owner, |generation| {
            generation["roles"][role]["tree"] = tree.clone();
            generation["roles"][role]["state"] = json!("running");
            Ok(())
        })?;
        let mut probe =
            json!({"role":role,"url":prepared.request["authority"][format!("{role}_endpoint")]});
        if role == "frontend" {
            probe["files"] = prepared.frontend_files.clone();
        }
        readiness::wait(&tree, &probe, options.timeout, || owner.check())?;
    }
    check(
        recapture(backend, options, owner, inputs)? == *prepared,
        "恢复运行启动期间来源、配置或工具发生变化",
    )?;
    check_live(state, owner)?;
    let launch = state.publish_launch(owner)?;
    inputs.finish(Ok(()))?;
    check_live(state, owner)?;
    Ok(launch)
}

fn check_live(state: &State, owner: &Owner) -> Result<()> {
    owner.check()?;
    let generation = state.generation();
    for role in ROLES {
        let tree = &generation["roles"][role]["tree"];
        for field in ["supervisor", "process", "monitor"] {
            actor::require_identity(&tree[field])?;
        }
        let endpoint = crate::recovery::inputs::domain::endpoint(
            &generation["request"]["authority"][format!("{role}_endpoint")],
            "恢复启动最终角色端点",
            None,
        )?;
        check(
            crate::recovery::fixture_control::services::lifecycle::readonly::process::start_listener_at(
                &tree["process"], endpoint.host.parse()?, endpoint.port,
            )?,
            "恢复启动最终核验缺少原角色监听",
        )?;
    }
    owner.check()
}

fn cleanup(
    backend: &Path,
    owner: &Owner,
    state: &mut State,
    attempts: &mut [(&'static str, launch::Attempt)],
    error: Box<dyn std::error::Error>,
) -> Box<dyn std::error::Error> {
    let mut generation = state.generation().clone();
    let mut failures = Vec::new();
    for role in ROLES.into_iter().rev() {
        let stopped = (|| {
            let observed = match attempts.iter_mut().find(|(name, _)| *name == role) {
                Some((_, attempt)) => attempt.cancel(owner)?,
                None => json!({"state":"pending","tree":null}),
            };
            stop::stop_role(
                backend,
                state.directory(),
                &mut generation,
                role,
                &observed,
                || owner.check(),
            )
        })();
        if let Err(failure) = stopped {
            failures.push(format!("{role}：{failure}"));
        }
    }
    generation["status"] = json!(if failures.is_empty() {
        "start_failed"
    } else {
        "cleanup_failed"
    });
    generation["error_type"] = json!("RuntimeStartError");
    if let Err(failure) = state.update(owner, |current| {
        *current = generation;
        Ok(())
    }) {
        failures.push(format!("生命周期失败证据：{failure}"));
    }
    if failures.is_empty() {
        error
    } else {
        format!(
            "{error}；恢复运行启动失败后的原树回收未完全确认：{}",
            failures.join("；")
        )
        .into()
    }
}
