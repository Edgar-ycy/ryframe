use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{FIXTURE_RUNTIME_USAGE, FixtureRuntimeCommand},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

#[path = "fixture_runtime/binding.rs"]
mod binding;
#[path = "fixture_runtime/browser.rs"]
mod browser;
#[path = "fixture_runtime/build.rs"]
mod build;
#[path = "fixture_runtime/dataset.rs"]
mod dataset;
#[path = "fixture_runtime/dependency.rs"]
mod dependency;

pub(in crate::recovery) fn dataset(root: &Path, operation: &str, args: &Value) -> Result<Value> {
    dataset::run(root, operation, args)
}
#[path = "fixture_runtime/execution.rs"]
mod execution;
#[path = "fixture_runtime/frontend.rs"]
mod frontend;

pub(in crate::recovery) fn frontend_runtime_contract(
    backend: &Path,
    output: &Path,
) -> Result<Value> {
    frontend::contract(backend, output)
}
pub(in crate::recovery) fn verify_frontend_handoff(directory: &Path, guard: &Value) -> Result<()> {
    frontend::handoff(directory, guard)
}
#[path = "fixture_runtime/manifest.rs"]
mod manifest;
#[path = "fixture_runtime/model.rs"]
mod model;
#[path = "fixture_runtime/plan.rs"]
mod plan;
#[path = "fixture_runtime/process.rs"]
mod process;
#[cfg(test)]
#[path = "../../tests/unit/recovery/fixture_runtime/process_tests.rs"]
mod process_tests;
#[path = "fixture_runtime/protocol.rs"]
pub(crate) mod protocol;
#[path = "fixture_runtime/publication.rs"]
mod publication;
#[path = "fixture_runtime/receipts.rs"]
mod receipts;
#[path = "fixture_runtime/responses.rs"]
mod responses;
#[path = "fixture_runtime/review.rs"]
mod review;
#[path = "fixture_runtime/runtime.rs"]
mod runtime;
#[cfg(test)]
#[path = "../../tests/unit/recovery/fixture_runtime/scenario_tests.rs"]
mod scenario_tests;
#[cfg(test)]
#[path = "../../tests/unit/recovery/fixture_runtime/tests.rs"]
mod tests;
#[path = "fixture_runtime/worker_handoff.rs"]
mod worker_handoff;
#[path = "fixture_runtime/worker_origin.rs"]
mod worker_origin;

pub(super) fn run(command: &FixtureRuntimeCommand, root: &Path) -> Result<()> {
    if matches!(command, FixtureRuntimeCommand::Help) {
        println!("{FIXTURE_RUNTIME_USAGE}");
        return Ok(());
    }
    validate_runtime(command, root)?;
    let value = match command {
        FixtureRuntimeCommand::Build(paths) => build::run(root, &paths.environment, &paths.output)?,
        FixtureRuntimeCommand::Verify(paths) => runtime::verify(
            &json!({"backend":root,"environment":paths.environment,"output":paths.output}),
        )?,
        FixtureRuntimeCommand::Start(paths)
        | FixtureRuntimeCommand::Stop(paths)
        | FixtureRuntimeCommand::Status(paths) => runtime::control(
            &json!({"backend":root,"environment":paths.environment,"output":paths.output,
                "action":command.operation()}),
            false,
            false,
        )?,
        FixtureRuntimeCommand::Bind { .. }
        | FixtureRuntimeCommand::Browser(_)
        | FixtureRuntimeCommand::BrowserVerify(_)
        | FixtureRuntimeCommand::BrowserClose(_) => return run_browser(command, root),
        FixtureRuntimeCommand::Help => return Err("帮助不执行夹具运行阶段".into()),
    };
    println!("{value}");
    Ok(())
}

fn run_browser(command: &FixtureRuntimeCommand, root: &Path) -> Result<()> {
    let credentials = if matches!(command, FixtureRuntimeCommand::Browser(_)) {
        Some(browser::CredentialBroker::start(
            &command
                .runtime()
                .ok_or("浏览器操作缺少运行路径")?
                .environment,
        )?)
    } else {
        None
    };
    let request = browser_request(
        command,
        root,
        credentials
            .as_ref()
            .map(browser::CredentialBroker::endpoint),
    )?;
    let mut api = dependency::Native::new(root)?;
    let result = execution::execute(&mut api, &request);
    if let Some(credentials) = credentials {
        let broker = credentials.finish(result.is_ok());
        if result.is_ok() {
            broker?;
        }
    }
    println!("{}", result?);
    Ok(())
}

fn browser_request(
    command: &FixtureRuntimeCommand,
    root: &Path,
    credential_endpoint: Option<&str>,
) -> Result<Value> {
    let runtime = command.runtime().ok_or("浏览器操作缺少运行路径")?;
    let mut request = json!({
        "backend_dir": path_text(root, "后端目录")?,
        "environment": path_text(&runtime.environment, "环境收据")?,
        "format_version": 1,
        "operation": command.operation(),
        "output": path_text(&runtime.output, "运行目录")?,
        "write": command.writes(),
        "browser_binding": path_text(command.browser_binding().ok_or("浏览器操作缺少绑定")?, "浏览器绑定")?,
    });
    if let Some(endpoint) = credential_endpoint {
        request["credential_endpoint"] = json!(endpoint);
    }
    if let FixtureRuntimeCommand::Bind { run_id, server, .. } = command {
        request["run_id"] = json!(run_id);
        request["server"] = json!(server.as_str());
    }
    Ok(request)
}

fn validate_runtime(command: &FixtureRuntimeCommand, root: &Path) -> Result<()> {
    let runtime = command
        .runtime()
        .ok_or("fixture runtime 帮助没有运行路径")?;
    validate_path(&runtime.environment, root, LocalTestPathKind::ExistingFile)?;
    let output_kind = if matches!(command, FixtureRuntimeCommand::Build(_)) {
        LocalTestPathKind::NewDirectory
    } else {
        LocalTestPathKind::ExistingDirectory
    };
    validate_path(&runtime.output, root, output_kind)?;
    if let Some(binding) = command.browser_binding() {
        let binding_kind = if matches!(command, FixtureRuntimeCommand::Bind { .. }) {
            LocalTestPathKind::OutputFile
        } else {
            LocalTestPathKind::ExistingFile
        };
        validate_path(binding, root, binding_kind)?;
        if binding.parent() != Some(runtime.output.as_path()) {
            return Err("浏览器绑定必须直接位于本次运行目录".into());
        }
    }
    Ok(())
}

fn validate_path(value: &Path, root: &Path, kind: LocalTestPathKind) -> Result<()> {
    validate_local_test_path(value, root, kind)
        .map(|_| ())
        .map_err(Into::into)
}

fn path_text<'a>(value: &'a Path, label: &str) -> Result<&'a str> {
    value
        .to_str()
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8").into())
}
