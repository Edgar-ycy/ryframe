use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{FIXTURE_RUNTIME_USAGE, FixtureRuntimeCommand, FixtureServer},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_env_removed,
};

const SCRIPT: &str = "tools/python/reference_fixture_runtime.py";

#[path = "fixture_runtime/binding.rs"]
mod binding;
#[path = "fixture_runtime/browser.rs"]
mod browser;
#[path = "fixture_runtime/dependency.rs"]
mod dependency;
#[path = "fixture_runtime/execution.rs"]
mod execution;
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
#[cfg(test)]
#[path = "../../tests/unit/recovery/fixture_runtime/scenario_tests.rs"]
mod scenario_tests;
#[cfg(test)]
#[path = "../../tests/unit/recovery/fixture_runtime/tests.rs"]
mod tests;
#[path = "fixture_runtime/worker_handoff.rs"]
mod worker_handoff;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateFixtureRuntimeInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &FixtureRuntimeCommand, root: &Path) -> Result<()> {
    if matches!(command, FixtureRuntimeCommand::Help) {
        println!("{FIXTURE_RUNTIME_USAGE}");
        return Ok(());
    }
    let _native_tool = crate::check::python_suite::native_tool_binding()?;
    let credentials = if matches!(command, FixtureRuntimeCommand::Browser(_)) {
        Some(browser::CredentialBroker::start(
            &command
                .runtime()
                .ok_or("fixture runtime 浏览器操作缺少运行路径")?
                .environment,
        )?)
    } else {
        None
    };
    let endpoint = credentials
        .as_ref()
        .map(browser::CredentialBroker::endpoint);
    let invocation = match endpoint {
        Some(endpoint) => private_invocation_with_endpoint(command, root, Some(endpoint))?,
        None => private_invocation_at(command, root)?,
    };
    if ["bind", "browser", "browser-verify", "browser-close"].contains(&command.operation()) {
        let mut request: Value = serde_json::from_str(&invocation.protocol)?;
        // 工具绑定只属于 Python 传输协议；原生浏览器协议保持原有字段。
        request
            .as_object_mut()
            .ok_or("夹具运行时请求必须是对象")?
            .remove("native_tool");
        let mut api = dependency::Python::new(root)?;
        let result = execution::execute(&mut api, &request);
        if let Some(credentials) = credentials {
            let broker = credentials.finish(result.is_ok());
            if result.is_ok() {
                broker?;
            }
        }
        println!("{}", result?);
        return Ok(());
    }
    let result = run_with_env_removed(
        root,
        "python",
        &[
            "-B",
            invocation.script,
            "--protocol-json",
            invocation.protocol.as_str(),
        ],
        &[],
        &[],
    );
    if let Some(credentials) = credentials {
        let broker = credentials.finish(result.is_ok());
        if result.is_ok() {
            broker?;
        }
    }
    result
}

pub(crate) fn private_invocation_at(
    command: &FixtureRuntimeCommand,
    root: &Path,
) -> Result<PrivateFixtureRuntimeInvocation> {
    private_invocation_with_endpoint(command, root, None)
}

fn private_invocation_with_endpoint(
    command: &FixtureRuntimeCommand,
    root: &Path,
    credential_endpoint: Option<&str>,
) -> Result<PrivateFixtureRuntimeInvocation> {
    let runtime = command
        .runtime()
        .ok_or("fixture runtime 帮助不启动私有阶段程序")?;
    validate_runtime(command, root)?;
    let mut protocol = json!({
        "backend_dir": path_text(root, "后端目录")?,
        "environment": path_text(&runtime.environment, "环境收据")?,
        "format_version": 1,
        "native_tool": crate::check::python_suite::native_tool_binding()?,
        "operation": command.operation(),
        "output": path_text(&runtime.output, "运行目录")?,
        "write": command.writes(),
    });
    if let Some(binding) = command.browser_binding() {
        protocol["browser_binding"] = json!(path_text(binding, "浏览器绑定")?);
    }
    if let Some(endpoint) = credential_endpoint {
        if !matches!(command, FixtureRuntimeCommand::Browser(_)) {
            return Err("浏览器凭据端点只允许用于 browser 操作".into());
        }
        protocol["credential_endpoint"] = json!(endpoint);
    }
    if let FixtureRuntimeCommand::Bind { run_id, server, .. } = command {
        protocol["run_id"] = json!(run_id);
        protocol["server"] = json!(server_text(*server));
    }
    Ok(PrivateFixtureRuntimeInvocation {
        script: SCRIPT,
        protocol: serialize_protocol(protocol)?,
    })
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

const fn server_text(server: FixtureServer) -> &'static str {
    server.as_str()
}

fn serialize_protocol(protocol: Value) -> Result<String> {
    let serialized = serde_json::to_string(&protocol)?;
    if serialized.contains(['\n', '\r', '\0']) {
        return Err("fixture runtime 私有协议不能包含换行符或 NUL".into());
    }
    Ok(serialized)
}
