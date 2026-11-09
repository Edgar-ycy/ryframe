//! 服务生命周期的唯一执行编排；OS 服务原语由受限运行时提供。
use super::history;
use crate::{
    Result,
    recovery::clone::native::{
        json as json_io,
        lifecycle::{
            lock::{ProcessGuard, RunLock},
            state,
        },
    },
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[path = "lifecycle/cache_primitive.rs"]
mod cache_primitive;
#[path = "lifecycle/close.rs"]
mod close;
#[path = "lifecycle/context.rs"]
mod context;
#[path = "lifecycle/controller_recovery.rs"]
mod controller_recovery;
#[path = "lifecycle/ledger.rs"]
mod ledger;
#[path = "lifecycle/model.rs"]
pub(super) mod model;
#[path = "lifecycle/observation.rs"]
mod observation;
#[path = "lifecycle/partial.rs"]
pub(super) mod partial;
#[path = "lifecycle/prepared.rs"]
mod prepared;
#[path = "lifecycle/primitive.rs"]
mod primitive;
#[path = "lifecycle/projection.rs"]
mod projection;
#[path = "lifecycle/protocol.rs"]
pub(crate) mod protocol;
#[path = "lifecycle/query.rs"]
pub(crate) mod query;
#[path = "lifecycle/readonly.rs"]
pub(crate) mod readonly;
#[path = "lifecycle/reconcile.rs"]
mod reconcile;
#[path = "lifecycle/reconcile_close.rs"]
mod reconcile_close;
#[path = "lifecycle/recover.rs"]
mod recover;
#[path = "lifecycle/registration.rs"]
mod registration;
#[path = "lifecycle/restart.rs"]
mod restart;
#[path = "lifecycle/runtime.rs"]
pub(crate) mod runtime;
#[path = "lifecycle/status.rs"]
mod status;
#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/services/lifecycle/tests.rs"]
mod tests;
#[path = "lifecycle/tree_control.rs"]
pub(super) mod tree_control;
#[cfg(windows)]
pub(in crate::recovery) use tree_control::observe_recovery_process;

use model::{Context, binding, check, read, text};

pub(super) trait Runtime {
    fn call(&mut self, operation: &str, arguments: Value) -> Result<Value>;
    fn now(&self) -> Duration;
    fn pause(&mut self, delay: Duration);
}

pub(crate) fn execute(
    backend: &Path,
    operation: &str,
    review: &Path,
    environment: &Path,
    owner: Option<&Path>,
    write: bool,
) -> Result<Value> {
    let outcome = (|| {
        if operation == "status" {
            return execute_with(
                &mut readonly::NativeRuntime::new(backend),
                backend,
                operation,
                review,
                environment,
                owner,
                write,
            );
        }
        let mut runtime = runtime::LifecycleRuntime::new(backend)?;
        execute_with(
            &mut runtime,
            backend,
            operation,
            review,
            environment,
            owner,
            write,
        )
    })();
    outcome.map_err(|failure: Box<dyn std::error::Error>| {
        if model::reason(failure.as_ref()).is_some()
            || crate::recovery::clone::native::cache::policy_message(failure.as_ref()).is_some()
            || crate::recovery::clone::native::cache::primitive_message(failure.as_ref()).is_some() {
            failure
        } else {
            model::error("夹具生命周期未完成；检查绑定配置、来源摘要、完整账本和进程证据，保留现场，不自动重放")
        }
    })
}

pub(super) fn execute_with(
    runtime: &mut dyn Runtime,
    backend: &Path,
    operation: &str,
    review: &Path,
    environment: &Path,
    owner: Option<&Path>,
    write: bool,
) -> Result<Value> {
    check(
        if operation == "status" { !write } else { write },
        "服务状态只读；生命周期写入必须显式指定 --write",
    )?;
    check(
        ["status", "close", "reconcile", "recover", "restart"].contains(&operation),
        "未知夹具生命周期操作",
    )?;
    check(
        (["recover", "reconcile", "restart"].contains(&operation)) == owner.is_some(),
        "recover/reconcile/restart 必须绑定明确 owner；close 不接受 owner",
    )?;
    let value = runtime.call(
        "context",
        json!({"review":review,"environment":environment}),
    )?;
    let context = Context::load(backend, value)?;
    match operation {
        "status" => status::run(runtime, &context),
        "close" => close::run(runtime, &context),
        "reconcile" => reconcile::run(runtime, &context, owner.ok_or("和解缺少 owner")?),
        "recover" => recover::run(runtime, &context, owner.ok_or("恢复缺少 owner")?),
        "restart" => restart::run(runtime, &context, owner.ok_or("重启缺少 owner")?),
        _ => unreachable!("已校验生命周期操作"),
    }
}

fn result(context: &Context, operation: &str, status: &str, details: Value) -> Result<Value> {
    let mut value = json!({"format_version":1,"kind":"reference-fixture-service-lifecycle",
        "operation":operation,"status":status,"run":context.run,"remote_writes":0,"resources_deleted":false});
    for (key, item) in details.as_object().ok_or("生命周期结果必须为对象")? {
        value[key] = item.clone();
    }
    Ok(value)
}

fn controller(runtime: &mut dyn Runtime, context: &Context) -> Result<Value> {
    runtime.call("controller", json!({"run":context.run}))
}

fn registered(runtime: &mut dyn Runtime, context: &Context, settled: bool) -> Result<Value> {
    runtime.call(
        "registered",
        json!({"context":context.value,"settled":settled}),
    )
}

fn observe(runtime: &mut dyn Runtime, services: &Value) -> Result<Value> {
    runtime.call("observe", json!({"services":services}))
}

fn expected_stopped() -> Value {
    json!({"redis":"stopped","rustfs":"stopped","termination":null})
}
fn expected_running() -> Value {
    json!({"redis":"running","rustfs":"running","termination":null})
}

fn owned<T>(context: &Context, body: impl FnOnce(&RunLock) -> Result<T>) -> Result<T> {
    let owner = RunLock::claim(&context.run)?;
    let outcome = body(&owner);
    let released = owner.release();
    match (outcome, released) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (_, Err(_)) => Err(model::error("控制锁释放未确认；保留账本与现场，不自动重放")),
    }
}

fn stage(
    context: &Context,
    operation: &str,
    sources: &Value,
    owner: &RunLock,
    body: impl FnOnce(u64, &Value, &Value, &Path) -> Result<Value>,
) -> Result<Value> {
    owner.verify()?;
    let (number, attempt) = ledger::begin(&context.run, operation, sources)?;
    state::bind_controller(&context.run, number, &owner.owner, &attempt)?;
    let controller = binding(&state::controller_path(&context.run, number))?;
    let running = binding(&context.run.join("state.json"))?;
    let output = context.run.join(format!("lifecycle-{number:04}"));
    let outcome = body(number, &controller, &running, &output);
    match outcome {
        Ok(value) => {
            ledger::finish(&context.run, number, Some(&value), None)?;
            Ok(value)
        }
        Err(error) => {
            let category = model::category(error.as_ref());
            if ledger::finish(&context.run, number, None, Some(category)).is_err() {
                return Err(model::error(
                    "生命周期失败且账本收尾未确认；保留完整前后像，不自动重放",
                ));
            }
            Err(error)
        }
    }
}

fn new_output(context: &Context, output: &Path) -> Result<()> {
    crate::local_test_path::validate_local_test_path(
        output,
        &context.backend,
        crate::local_test_path::LocalTestPathKind::NewDirectory,
    )?;
    std::fs::create_dir(output)?;
    Ok(())
}

fn service_projection(observations: &Value) -> Value {
    json!({"redis":observations["redis"],"rustfs":observations["rustfs"]})
}

fn check_observation(
    runtime: &mut dyn Runtime,
    services: &Value,
    expected: &Value,
    message: &'static str,
) -> Result<()> {
    check(observe(runtime, services)? == *expected, message)
}
