//! 正式初始化与显式续作；外层登记锁和真实目标Generation覆盖完整资源窗口。
mod claims;
pub(crate) mod initial;
pub(crate) mod inputs;
mod marker;
pub(crate) mod operations;
pub(crate) mod reset_configuration;
pub(crate) mod resume;
pub(crate) mod window;
use super::super::super::{
    bridge::scope::Scope,
    export::transport::{SystemTransport, Transport},
    json as document,
    resources::common,
};
use super::super::{guard::Generation, resume_baseline, tree};
use super::{
    receipts,
    registration::{Registration, Workspace, combine},
    resources::Resources,
};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;
use window::Window;

pub(crate) fn execute(backend: &Path, workspace: &Path, resuming: bool) -> Result<Value> {
    let scope = Scope::load(backend, &[], &[])?;
    let registration = Registration::load(backend, workspace)?;
    let control = Workspace::claim(backend, workspace)?;
    let result = registration
        .execute(|active| run(&registration, &scope, active, resuming, &SystemTransport));
    combine(control.finish(result), scope.assert_unchanged())
}
pub(crate) fn prepared(registration: &Registration) -> Result<Value> {
    let path = registration.workspace.join("prepared-files.json");
    let value = document::read(&path)?;
    let descriptor = json!({"descriptor":common::binding(&path)?,"registration":registration.binding,"predecessor":null});
    resume_baseline::prepared(
        &registration.backend,
        &registration.target,
        &descriptor,
        &registration.value["request"],
    )?;
    if value["registration"] != registration.binding {
        return Err("准备快照不属于当前工作区".into());
    }
    Ok(descriptor)
}
fn run<T: Transport>(
    registration: &Registration,
    scope: &Scope,
    active: Option<&Path>,
    resuming: bool,
    transport: &T,
) -> Result<Value> {
    let proof = prepared(registration)?;
    let request = common::read_bound(
        &registration.backend,
        &registration.value["request"],
        "初始化固定请求",
    )?;
    let environment = super::environment::registered(registration, &request)?;
    let protected = inputs::protect(registration, &request, &environment)?;
    let state = if resuming {
        Some(resume::state(registration, &proof, false)?)
    } else {
        None
    };
    if !resuming
        && registration
            .target
            .join("initialize.started.json")
            .try_exists()?
    {
        return Err("本代次已有执行记录；只允许核对或明确续作".into());
    }
    let mut attempt = state
        .map(|state| resume::Attempt::new(registration, state))
        .transpose()?;
    let owner = Generation::acquire(&registration.backend, &registration.target)?;
    let inputs = super::super::storage::StorageInputs {
        backend: &registration.backend,
        request: &request,
        environment: &environment,
        observation_dir: &registration.target,
    };
    let mut stage = "initialize_binding";
    let result = {
        let resources = Resources {
            inputs: super::super::storage::StorageInputs {
                backend: inputs.backend,
                request: inputs.request,
                environment: inputs.environment,
                observation_dir: inputs.observation_dir,
            },
            transport,
            scope,
            run: active,
        };
        let observe_generation = |publish| resources.generation_owned(Some(&owner), publish);
        let observer = super::super::storage::SystemObserver::new(transport);
        let window = Window {
            registration,
            inputs,
            transport,
            owner: &owner,
            observe_generation: &observe_generation,
            observer: &observer,
            protected: &protected,
        };
        if let Some(attempt) = attempt.as_mut() {
            resume::execute(&window, &proof, attempt)
        } else {
            initial::execute(&window, &proof, &mut stage)
        }
    };
    let result = owner.finish_publishing(result, |(_, files)| publish(registration, files, &proof));
    let result = combine(result, protected.check());
    finish(registration, resuming, attempt.as_mut(), stage, result)
}
pub(crate) fn finish(
    registration: &Registration,
    resuming: bool,
    attempt: Option<&mut resume::Attempt>,
    stage: &str,
    result: Result<(Value, Value)>,
) -> Result<Value> {
    match result {
        Ok((value, _)) => {
            let mut result = json!({"status":value["status"],"registration":registration.binding,
                "target":common::binding(&registration.target.join("initialized.json"))?,
                "files":common::binding(&registration.workspace.join("initialized-files.json"))?,"restore_qualified":false});
            if resuming {
                result["resumed"] = json!(true);
            }
            Ok(result)
        }
        Err(error) => {
            if !resuming {
                let check = initial::failure(registration, stage, error.as_ref());
                return combine(Err(error), check);
            }
            let check = attempt
                .ok_or("续作失败缺少尝试前像")?
                .failed(registration, error.as_ref());
            combine(Err(error), check)
        }
    }
}
pub(crate) fn publish(registration: &Registration, files: &Value, proof: &Value) -> Result<()> {
    registration.check(true)?;
    resume_baseline::prepared(
        &registration.backend,
        &registration.target,
        proof,
        &registration.value["request"],
    )?;
    if registration.target.join("initialize.lock").try_exists()? {
        return Err("初始化目录锁未释放，不能发布快照".into());
    }
    if tree::snapshot(&registration.backend, &registration.target, true)? != *files {
        return Err("初始化释放后的完整目标树不同于执行结果".into());
    }
    receipts::write(
        &registration.workspace.join("initialized-files.json"),
        &json!({"format_version":1,
        "kind":"devex-clone-fresh-target-initialized-files","registration":registration.binding,
        "predecessor":proof["descriptor"],"files":files}),
    )?;
    if tree::snapshot(&registration.backend, &registration.target, true)? != *files {
        return Err("初始化快照发布期间完整树变化".into());
    }
    resume_baseline::prepared(
        &registration.backend,
        &registration.target,
        proof,
        &registration.value["request"],
    )?;
    registration.check(true)
}
