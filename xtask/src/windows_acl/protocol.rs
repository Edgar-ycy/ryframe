use std::{
    io::Write,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::{Descriptor, ERROR, native};
use crate::Result;

const MARKER: &str = "__windows-acl";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    result: Option<Descriptor>,
    error: Option<String>,
}

pub(crate) fn read(path: &Path) -> Result<Descriptor> {
    call("read", path)
}

pub(crate) fn secure_directory(path: &Path) -> Result<()> {
    call("secure-directory", path)?
        .verify_mysql(true)
        .map_err(Into::into)
}

fn call(operation: &str, path: &Path) -> Result<Descriptor> {
    let system = native::system_directory()?;
    let mut command = Command::new(executable()?);
    command.args([MARKER, operation]).arg(path);
    command
        .env_clear()
        .env("SystemRoot", system.parent().ok_or(ERROR)?);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.creation_flags(0x0800_0000);
    let output =
        crate::process::command_output_result_with_timeout(command, Duration::from_secs(10))
            .map_err(|_| ERROR)?;
    if !output.stderr.is_empty() || output.stdout.len() > 1024 * 1024 {
        return Err(ERROR.into());
    }
    let response: Response = serde_json::from_slice(&output.stdout).map_err(|_| ERROR)?;
    match (output.status.success(), response) {
        (
            true,
            Response {
                result: Some(value),
                error: None,
            },
        ) => Ok(value),
        (
            false,
            Response {
                result: None,
                error: Some(error),
            },
        ) if error.starts_with("Windows ACL") && error.len() <= 512 => Err(error.into()),
        _ => Err(ERROR.into()),
    }
}

fn executable() -> Result<PathBuf> {
    #[cfg(test)]
    let path = PathBuf::from(
        option_env!("CARGO_BIN_EXE_xtask")
            .ok_or("Windows ACL 测试必须由唯一 internal 入口绑定真实 xtask 程序")?,
    );
    #[cfg(not(test))]
    let path = std::env::current_exe()?;
    if !path.is_absolute() || !path.is_file() {
        return Err(ERROR.into());
    }
    Ok(path)
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if arguments.first().is_none_or(|argument| argument != MARKER) {
        return None;
    }
    let result: Result<Descriptor> = match arguments.as_slice() {
        [_, operation, path] => operation
            .to_str()
            .filter(|operation| matches!(*operation, "read" | "secure-directory"))
            .ok_or_else(|| ERROR.into())
            .and_then(|operation| native::execute(operation, Path::new(path))),
        _ => Err(ERROR.into()),
    };
    let (code, response) = match result {
        Ok(result) => (
            0,
            Response {
                result: Some(result),
                error: None,
            },
        ),
        Err(error) => (
            1,
            Response {
                result: None,
                error: Some(error.to_string()),
            },
        ),
    };
    let written = serde_json::to_vec(&response)
        .map_err(std::io::Error::other)
        .and_then(|mut bytes| {
            bytes.push(b'\n');
            std::io::stdout().lock().write_all(&bytes)
        });
    Some(if written.is_ok() { code } else { 1 })
}
