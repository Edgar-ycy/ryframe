use std::path::Path;

use serde_json::{Value, json};

use crate::{
    Result,
    cli::PerformanceIdentitiesCommand,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_env_removed,
    workspace::root_dir,
};

const SCRIPT: &str = "scripts/devex_prepare_identities.mjs";
const PROTOCOL_KEY: &str = "RYFRAME_PERFORMANCE_IDENTITIES_PROTOCOL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateIdentityInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &PerformanceIdentitiesCommand) -> Result<()> {
    let root = root_dir();
    let invocation = private_invocation_at(command, &root)?;
    run_with_env_removed(
        &root,
        "node",
        &[invocation.script],
        &[(PROTOCOL_KEY, invocation.protocol.as_str())],
        &[PROTOCOL_KEY],
    )
}

pub(crate) fn private_invocation_at(
    command: &PerformanceIdentitiesCommand,
    root: &Path,
) -> Result<PrivateIdentityInvocation> {
    let protocol = match command {
        PerformanceIdentitiesCommand::Plan {
            environment,
            output,
        } => {
            validate_path(environment, root, LocalTestPathKind::ExistingFile)?;
            validate_path(output, root, LocalTestPathKind::OutputFile)?;
            json!({
                "format_version": 1,
                "operation": "plan",
                "environment": path_text(environment)?,
                "output": path_text(output)?,
                "write": true,
            })
        }
        PerformanceIdentitiesCommand::Apply { plan, state_dir }
        | PerformanceIdentitiesCommand::Verify { plan, state_dir } => {
            validate_path(plan, root, LocalTestPathKind::ExistingFile)?;
            validate_path(state_dir, root, LocalTestPathKind::StateDirectory)?;
            json!({
                "format_version": 1,
                "operation": command.operation(),
                "plan": path_text(plan)?,
                "state_dir": path_text(state_dir)?,
                "write": true,
            })
        }
    };
    Ok(PrivateIdentityInvocation {
        script: SCRIPT,
        protocol: serialize_protocol(protocol)?,
    })
}

fn validate_path(value: &Path, root: &Path, kind: LocalTestPathKind) -> Result<()> {
    validate_local_test_path(value, root, kind)
        .map(|_| ())
        .map_err(Into::into)
}

fn path_text(value: &Path) -> Result<&str> {
    value
        .to_str()
        .ok_or_else(|| "性能身份路径必须能表示为 UTF-8".into())
}

fn serialize_protocol(protocol: Value) -> Result<String> {
    let serialized = serde_json::to_string(&protocol)?;
    if serialized.contains(['\n', '\r', '\0']) {
        return Err("性能身份私有协议不能包含换行符或 NUL".into());
    }
    Ok(serialized)
}
