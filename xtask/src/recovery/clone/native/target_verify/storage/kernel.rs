use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::Value;

use super::{TargetContext, Transport, common};
use crate::Result;

pub(super) fn require_identity(identity: &Value) -> Result<()> {
    check_identity(identity, common::process_identity)
}

pub(super) fn require_observed_identity(
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
    context: &TargetContext,
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
            cwd: context.backend.clone(),
            environment: context.environment.clone(),
            stdin: None,
            stdout: StdoutTarget::Capture,
            work_dir: context.observation_dir.clone(),
            label: label.into(),
            timeout: Duration::from_secs(15),
        })
        .map_err(|_| "存储固定只读观察命令失败")?;
    if !output.status.success() {
        return Err("存储固定只读观察命令失败".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "存储观察输出不是 UTF-8".into())
}

pub(super) fn powershell(context: &TargetContext) -> Result<PathBuf> {
    let root = context
        .environment
        .get("SystemRoot")
        .cloned()
        .or_else(|| std::env::var("SystemRoot").ok())
        .ok_or("Windows 缺少 SystemRoot")?;
    Ok(Path::new(&root).join("System32/WindowsPowerShell/v1.0/powershell.exe"))
}

pub(super) fn argv<T: Transport>(
    context: &TargetContext,
    identity: &Value,
    transport: &T,
) -> Result<Vec<String>> {
    if !cfg!(windows) {
        return Err("RustFS 原生完整参数核验仅用于 Windows".into());
    }
    require_identity(identity)?;
    let pid = identity["pid"].as_u64().ok_or("RustFS PID 无效")?;
    let script = format!(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); (Get-CimInstance Win32_Process -Filter 'ProcessId = {pid}').CommandLine | ConvertTo-Json -Compress"
    );
    let raw = observe(
        context,
        transport,
        powershell(context)?,
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            script,
        ],
        "target-storage-argv",
    )?;
    require_identity(identity)?;
    let line: String = serde_json::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|_| "RustFS 参数观察结果无效")?;
    windows_argv(&line)
}

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

pub(super) fn listener<T: Transport>(
    context: &TargetContext,
    transport: &T,
    identity: &Value,
    url: &str,
) -> Result<()> {
    let (port, _) = common::endpoint(url)?;
    require_identity(identity)?;
    let script = format!(
        "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new($false); @(Get-NetTCPConnection -State Listen -LocalPort {port} -ErrorAction Stop | Select-Object LocalAddress,OwningProcess) | ConvertTo-Json -Compress"
    );
    let raw = observe(
        context,
        transport,
        powershell(context)?,
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            script,
        ],
        "target-storage-listener",
    )?;
    require_identity(identity)?;
    let listeners: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}'))
        .map_err(|_| "RustFS 监听观察结果无效")?;
    validate_listeners(&listeners, identity)
}

pub(crate) fn validate_listeners(listeners: &Value, identity: &Value) -> Result<()> {
    let rows = listeners.as_array().ok_or("RustFS 监听观察必须是列表")?;
    if !rows.iter().any(|row| {
        row.get("OwningProcess") == identity.get("pid")
            && matches!(row["LocalAddress"].as_str(), Some("127.0.0.1" | "0.0.0.0"))
    }) {
        return Err("RustFS 端口监听所有者不同于登记内核进程".into());
    }
    Ok(())
}
