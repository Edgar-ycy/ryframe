use std::{path::PathBuf, time::Duration};

use serde_json::Value;

use super::{StorageInputs, Transport, common};
use crate::Result;

pub(crate) fn require_observed_identity(
    identity: &Value,
    observer: &impl super::StorageObserver,
) -> Result<()> {
    check_identity(identity, |pid| observer.process_identity(pid))
}

fn check_identity(
    identity: &Value,
    observe: impl FnOnce(u32) -> Result<Option<Value>>,
) -> Result<()> {
    let identity = common::validate_process_identity(identity)?;
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("存储 PID 无效")?)
        .map_err(|_| "存储 PID 超出范围")?;
    if observe(pid)?.as_ref() != Some(&identity) {
        return Err("RustFS 观察窗口中的内核进程身份变化".into());
    }
    Ok(())
}

pub(crate) fn observe<T: Transport>(
    context: &StorageInputs<'_>,
    transport: &T,
    program: PathBuf,
    args: Vec<String>,
    label: &str,
) -> Result<String> {
    use super::super::super::export::transport::{CommandSpec, StdoutTarget};
    let output = transport
        .run(CommandSpec {
            program,
            args,
            cwd: context.backend.to_path_buf(),
            environment: context.environment.clone(),
            stdin: None,
            stdout: StdoutTarget::Capture,
            work_dir: context.observation_dir.to_path_buf(),
            label: label.into(),
            timeout: Duration::from_secs(15),
        })
        .map_err(|_| "存储固定只读观察命令失败")?;
    if !output.status.success() {
        return Err("存储固定只读观察命令失败".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "存储观察输出不是 UTF-8".into())
}

#[cfg(any(windows, test))]
pub(crate) fn windows_argv(line: &str) -> Result<Vec<String>> {
    if line.is_empty() || line.contains('\0') {
        return Err("RustFS 参数为空或包含 NUL".into());
    }
    let mut input = line.chars().peekable();
    let mut result = Vec::new();
    if line.starts_with([' ', '\t']) {
        result.push(String::new());
    }
    while input.peek().is_some() {
        while input.peek().is_some_and(|ch| matches!(ch, ' ' | '\t')) {
            input.next();
        }
        if input.peek().is_none() {
            break;
        }
        let mut argument = String::new();
        let mut quoted = false;
        while let Some(&ch) = input.peek() {
            if !quoted && matches!(ch, ' ' | '\t') {
                break;
            }
            let mut slashes = 0;
            while input.peek() == Some(&'\\') {
                slashes += 1;
                input.next();
            }
            if input.peek() == Some(&'"') {
                argument.extend(std::iter::repeat_n('\\', slashes / 2));
                input.next();
                if slashes % 2 == 1 {
                    argument.push('"');
                } else if quoted && input.peek() == Some(&'"') {
                    argument.push('"');
                    input.next();
                } else {
                    quoted = !quoted;
                }
            } else {
                argument.extend(std::iter::repeat_n('\\', slashes));
                if let Some(&next) = input.peek() {
                    if !quoted && matches!(next, ' ' | '\t') {
                        break;
                    }
                    argument.push(next);
                    input.next();
                }
            }
        }
        result.push(argument);
    }
    Ok(result)
}
