use std::path::Path;

use serde_json::{Value, json};

use super::{super::json::exact, manifest};
use crate::Result;

pub(crate) fn current() -> Result<Value> {
    process(std::process::id())?.ok_or_else(|| "无法取得验收控制进程的内核身份".into())
}

pub(crate) fn process(pid: u32) -> Result<Option<Value>> {
    if pid <= 1 {
        return Err("测试进程 PID 必须大于 1".into());
    }
    platform(pid)
}

pub(crate) fn validate(value: &Value, directory: &Path) -> Result<()> {
    let owner = exact(
        value,
        &["format_version", "identity", "directory", "manifest_sha256"],
        "控制器收据",
    )?;
    let identity = exact(
        owner.get("identity").unwrap(),
        &["pid", "started", "executable"],
        "控制器进程身份",
    )?;
    let directory = manifest::path_text(directory, "验收运行目录")?;
    if owner.get("format_version").and_then(Value::as_u64) != Some(1)
        || owner.get("directory").and_then(Value::as_str) != Some(directory.as_str())
        || identity
            .get("pid")
            .and_then(Value::as_u64)
            .is_none_or(|pid| pid <= 1)
        || ["started", "executable"].iter().any(|field| {
            identity
                .get(*field)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        })
    {
        return Err("控制器收据不属于固定运行目录或缺少完整进程身份".into());
    }
    Ok(())
}

pub(crate) fn same(expected: &Value, actual: Option<&Value>) -> Result<(bool, bool)> {
    let Some(actual) = actual else {
        return Ok((false, true));
    };
    Ok((expected == actual, false))
}

#[cfg(windows)]
fn platform(pid: u32) -> Result<Option<Value>> {
    use winsafe::{FILETIME, HPROCESS, co, prelude::kernel_Hprocess};

    let handle = match HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(co::ERROR::INVALID_PARAMETER) => return Ok(None),
        Err(error) => return Err(format!("无法打开进程 {pid}：{error}").into()),
    };
    if handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0 {
        return Ok(None);
    }
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    handle.GetProcessTimes(&mut creation, &mut exit, &mut kernel, &mut user)?;
    let executable = handle.QueryFullProcessImageName(co::PROCESS_NAME::WIN32)?;
    let executable = executable
        .strip_prefix(r"\\?\UNC\")
        .map(|value| format!(r"\\{value}"))
        .or_else(|| executable.strip_prefix(r"\\?\").map(ToOwned::to_owned))
        .unwrap_or(executable);
    let started = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
    Ok(Some(json!({
        "pid": pid,
        "started": started.to_string(),
        "executable": executable.replace('/', "\\"),
    })))
}

#[cfg(unix)]
fn platform(pid: u32) -> Result<Option<Value>> {
    let root = Path::new("/proc").join(pid.to_string());
    let stat = match std::fs::read_to_string(root.join("stat")) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取进程 {pid} 状态：{error}").into()),
    };
    let fields = stat
        .rsplit_once(')')
        .map(|(_, value)| value.split_whitespace().collect::<Vec<_>>())
        .ok_or("进程状态格式无效")?;
    let started = fields.get(19).ok_or("进程状态缺少创建身份")?;
    let executable = match std::fs::read_link(root.join("exe")) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !root.join("stat").exists() {
                return Ok(None);
            }
            return Err(format!("测试进程仍存在但可执行映像无法核验：{error}").into());
        }
        Err(error) => return Err(format!("无法读取进程 {pid} 可执行映像：{error}").into()),
    };
    Ok(Some(json!({
        "pid": pid,
        "started": started,
        "executable": executable.to_string_lossy(),
    })))
}

#[cfg(not(any(unix, windows)))]
fn platform(pid: u32) -> Result<Option<Value>> {
    if pid == std::process::id() {
        Ok(Some(json!({
            "pid": pid,
            "started": "unsupported",
            "executable": std::env::current_exe()?.to_string_lossy(),
        })))
    } else {
        Ok(None)
    }
}
