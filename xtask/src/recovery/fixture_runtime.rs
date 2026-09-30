use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::{FIXTURE_RUNTIME_USAGE, FixtureRuntimeCommand, FixtureServer},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_env_removed,
};

const SCRIPT: &str = "tools/python/reference_fixture_runtime.py";
const PROTOCOL_KEY: &str = "RYFRAME_REFERENCE_FIXTURE_RUNTIME_PROTOCOL";

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
    let invocation = private_invocation_at(command, root)?;
    run_with_env_removed(
        root,
        "python",
        &["-B", invocation.script],
        &[(PROTOCOL_KEY, invocation.protocol.as_str())],
        &[PROTOCOL_KEY],
    )
}

pub(crate) fn private_invocation_at(
    command: &FixtureRuntimeCommand,
    root: &Path,
) -> Result<PrivateFixtureRuntimeInvocation> {
    let runtime = command
        .runtime()
        .ok_or("fixture runtime 帮助不启动私有阶段程序")?;
    validate_runtime(command, root)?;
    let mut protocol = json!({
        "backend_dir": path_text(root, "后端目录")?,
        "environment": path_text(&runtime.environment, "环境收据")?,
        "format_version": 1,
        "operation": command.operation(),
        "output": path_text(&runtime.output, "运行目录")?,
        "write": command.writes(),
    });
    if let Some(binding) = command.browser_binding() {
        protocol["browser_binding"] = json!(path_text(binding, "浏览器绑定")?);
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
