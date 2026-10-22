use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    path::Path,
    process::Stdio,
};

use serde_json::{Value, json};

use crate::{
    Result,
    cli::PerformanceIdentitiesCommand,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_stdin,
    workspace::root_dir,
};

const SCRIPT: &str = "tools/js/devex_prepare_identities.mjs";
const MAX_PROTOCOL_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateIdentityInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &PerformanceIdentitiesCommand) -> Result<()> {
    let root = root_dir();
    let invocation = private_invocation_at(command, &root)?;
    // 使用有界且已到 EOF 的独立输入，不继承终端，也不把协议放进环境或日志。
    let input_directory = root.join(".local-tests");
    fs::create_dir_all(&input_directory)?;
    let mut input = tempfile::tempfile_in(input_directory)?;
    input.write_all(invocation.protocol.as_bytes())?;
    input.seek(SeekFrom::Start(0))?;
    run_with_stdin(&root, "node", &[invocation.script], Stdio::from(input))
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
    if serialized.len() > MAX_PROTOCOL_BYTES {
        return Err("性能身份私有协议超过 32 KiB 上限".into());
    }
    if serialized.contains(['\n', '\r', '\0']) {
        return Err("性能身份私有协议不能包含换行符或 NUL".into());
    }
    Ok(serialized)
}

#[cfg(test)]
mod tests {
    use super::{MAX_PROTOCOL_BYTES, serialize_protocol};
    use serde_json::Value;

    #[test]
    fn protocol_limit_counts_utf8_bytes_including_json_encoding() {
        let at_limit = Value::String("a".repeat(MAX_PROTOCOL_BYTES - 2));
        assert_eq!(
            serialize_protocol(at_limit).unwrap().len(),
            MAX_PROTOCOL_BYTES
        );
        let oversized = Value::String("密".repeat(MAX_PROTOCOL_BYTES / 3 + 1));
        let error = serialize_protocol(oversized)
            .expect_err("超限协议必须失败")
            .to_string();
        assert!(error.contains("32 KiB"));
        assert!(!error.contains('密'));
    }
}
