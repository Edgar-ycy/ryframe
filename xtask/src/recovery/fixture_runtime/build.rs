//! 正式 fixture Build：复用四角色 Cargo 构建和原有来源、配置及运行登记。
use super::model::*;
use crate::{
    Result,
    recovery::{
        clone::native::{bridge, digest, source},
        fixture_control, full_stack,
    },
};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

#[path = "build/configuration.rs"]
mod configuration;
#[path = "build/environment.rs"]
mod environment;
use environment::Inputs;

pub(super) fn run(backend: &Path, environment: &Path, output: &Path) -> Result<Value> {
    let inputs = Inputs::load(backend, environment)?;
    build(&inputs, output, &mut |program, args| {
        command(&inputs, program, args)
    })
}

fn build(
    inputs: &Inputs,
    output: &Path,
    command: &mut impl FnMut(&str, &[String]) -> Result<String>,
) -> Result<Value> {
    safe(output)?;
    let allowed = inputs.root.join(".local-tests/reference-fixture");
    check(
        output.starts_with(&allowed) && output != allowed,
        "夹具构建目录越界",
    )?;
    directory(output.parent().ok_or("夹具构建目录缺少父目录")?)?;
    std::fs::create_dir(output)?;
    let result = execute(inputs, output, command);
    if result.is_err() {
        // 不记录底层原文或配置；失败证据只新建，不能覆盖未知结果。
        write_new(
            &output.join("failure.json"),
            &json!({
                "status":"reference_fixture_runtime_failed","error_type":"NativeBuildError"
            }),
        )?;
    }
    result
}

fn execute(
    inputs: &Inputs,
    output: &Path,
    command: &mut impl FnMut(&str, &[String]) -> Result<String>,
) -> Result<Value> {
    inputs.verify()?;
    let pair = fixture_control::source_pair::execution::pair(
        &inputs.coordinator,
        &inputs.root,
        &inputs.execution_source,
    )?;
    write_new(&output.join("source-pair.json"), &pair)?;
    let original = full_stack::provenance::current(&inputs.root, output)?;
    let inventory = source::capture(&inputs.root)?;
    let context = context(inputs, command)?;
    let binaries = full_stack::build::run(&inputs.root, output, &mut |program, args| {
        inputs.verify()?;
        let result = command(program, args);
        inputs.verify()?;
        result
    })?;
    let backend = backend_build(&binaries, &inventory, &context)?;
    inputs.verify()?;
    check(
        source::capture(&inputs.root)? == inventory
            && self::context(inputs, command)? == context
            && full_stack::provenance::current(&inputs.root, output)? == original,
        "夹具构建期间完整来源或工具链上下文漂移；保留现场，不自动重放",
    )?;
    bridge::verify_build(&inputs.root, &output.join("backend-build.json"), &backend)?;
    write_new(&output.join("backend-build.json"), &backend)?;
    let (api, worker) = configuration::copy(&inputs.root, output, &inputs.configuration)?;
    inputs.verify()?;
    let receipt =
        fixture_control::artifact::runtime::register(&inputs.root, output, &api, &worker)?;
    inputs.verify()?;
    check(
        source::capture(&inputs.root)? == inventory,
        "夹具运行登记期间源码漂移",
    )?;
    bridge::verify_build(&inputs.root, &output.join("backend-build.json"), &backend)?;
    Ok(json!({"status":"reference_fixture_runtime_built",
        "source_pair":bound(&output.join("source-pair.json"))?,
        "backend_build":bound(&output.join("backend-build.json"))?,
        "runtime":bound(&output.join("runtime.json"))?,"scope_id":receipt["scope_id"],"remote_writes":0}))
}

fn backend_build(
    binaries: &Value,
    inventory: &source::Inventory,
    context: &Value,
) -> Result<Value> {
    let mut artifacts = serde_json::Map::new();
    for (role, name) in [("api", "ryframe"), ("worker", "ryframe-worker")] {
        let executable = path(&binaries[name])?;
        let file = digest::file(&executable)?;
        artifacts.insert(
            role.into(),
            json!({"executable":executable,
            "command":context["commands"][role],"bytes":file.bytes,"sha256":file.sha256}),
        );
    }
    Ok(json!({"format_version":2,"kind":"restore-backend-build",
        "sources":source::backend_domains(inventory)?,"build":context,"artifacts":artifacts}))
}

fn context(
    inputs: &Inputs,
    command: &mut impl FnMut(&str, &[String]) -> Result<String>,
) -> Result<Value> {
    inputs.verify()?;
    let cargo = command("cargo", &["-V".into()])?;
    let rustc = command("rustc", &["-vV".into()])?;
    inputs.verify()?;
    bridge::build_context::from_inputs(&cargo, &rustc, &inputs.environment)
}

fn command(inputs: &Inputs, program: &str, args: &[String]) -> Result<String> {
    let mut command = crate::process::child_command(program);
    command
        .args(args)
        .current_dir(&inputs.root)
        .env_clear()
        .envs(&inputs.environment);
    full_stack::execution::capture(
        command,
        &inputs.root,
        &[],
        Duration::from_secs(1800),
        None,
        false,
        false,
    )
}

#[cfg(test)]
#[path = "../../../tests/unit/recovery/fixture_runtime/build_tests.rs"]
mod tests;
