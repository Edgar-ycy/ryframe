use crate::Result;

#[cfg(windows)]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ArgumentRow {
    #[serde(rename = "ProcessId")]
    pid: u32,
    #[serde(rename = "CommandLine")]
    command: Option<String>,
}

#[cfg(windows)]
pub(crate) fn arguments(pid: u32) -> Result<Vec<String>> {
    observe_arguments(pid)?.ok_or_else(|| "进程参数不可读，不能用空参数放行".into())
}

#[cfg(windows)]
fn observe_arguments(pid: u32) -> Result<Option<Vec<String>>> {
    let rows = connect()?
        .raw_query(argument_query(pid))
        .map_err(|_| QUERY_ERROR)?;
    checked_arguments(pid, rows)
}

#[cfg(windows)]
pub(super) fn arguments_before(
    pid: u32,
    deadline: std::time::Instant,
) -> Result<Option<Vec<String>>> {
    use std::{sync::mpsc, thread, time::Instant};

    const TIMEOUT: &str = "取消测量读取编译进程参数超时";
    if Instant::now() >= deadline {
        return Err(TIMEOUT.into());
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    // 同步 CIM 调用可能阻塞；观察超时后不等待线程退出，调用方可以立即回收 Job。
    drop(thread::Builder::new().spawn(move || {
        let result = observe_arguments(pid).map_err(|error| error.to_string());
        let _ = sender.send(result);
    })?);

    let result = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    if Instant::now() >= deadline {
        return Err(TIMEOUT.into());
    }
    match result {
        Ok(result) => result.map_err(Into::into),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(TIMEOUT.into()),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err("编译进程参数观察线程未返回结果".into()),
    }
}

#[cfg(windows)]
const QUERY_ERROR: &str = "进程参数 CIM 查询失败；检查本机读取权限，保留现场";

#[cfg(windows)]
fn connect() -> Result<wmi::WMIConnection> {
    wmi::WMIConnection::new()
        .map_err(|_| "进程参数 CIM 连接失败；检查本机读取权限，不能推断进程已退出".into())
}

#[cfg(windows)]
fn argument_query(pid: u32) -> String {
    format!("SELECT ProcessId,CommandLine FROM Win32_Process WHERE ProcessId={pid}")
}

#[cfg(windows)]
fn checked_arguments(pid: u32, rows: Vec<ArgumentRow>) -> Result<Option<Vec<String>>> {
    let [row] = rows.as_slice() else {
        return Err("进程参数观察未返回唯一登记 PID".into());
    };
    if row.pid != pid {
        return Err("进程参数观察未返回唯一登记 PID".into());
    }
    row.command.as_deref().map(windows_argv).transpose()
}

pub(crate) fn windows_argv(line: &str) -> Result<Vec<String>> {
    if line.is_empty() || line.contains('\0') {
        return Err("Windows 进程参数为空或包含 NUL".into());
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
