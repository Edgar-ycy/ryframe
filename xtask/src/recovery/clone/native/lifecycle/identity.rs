use std::{error::Error, fmt, path::Path};

use serde_json::{Value, json};

use super::{super::json::exact, manifest};
use crate::Result;

#[derive(Debug)]
struct AccessDenied {
    pid: u32,
    operation: &'static str,
}

impl fmt::Display for AccessDenied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "进程 {} 的{}被拒绝访问（错误码5）",
            self.pid, self.operation
        )
    }
}

impl Error for AccessDenied {}

#[derive(Debug)]
struct ObservationFailure {
    observation: Box<dyn Error>,
    wait: Box<dyn Error>,
}

impl fmt::Display for ObservationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}；同一进程句柄的退出状态也无法核验：{}",
            self.observation, self.wait
        )
    }
}

impl Error for ObservationFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.observation.as_ref())
    }
}

pub(crate) fn is_access_denied(error: &(dyn Error + 'static)) -> bool {
    if error.is::<AccessDenied>() {
        return true;
    }
    if let Some(failure) = error.downcast_ref::<ObservationFailure>() {
        return is_access_denied(failure.observation.as_ref())
            || is_access_denied(failure.wait.as_ref());
    }
    error.source().is_some_and(is_access_denied)
}

#[cfg(windows)]
fn windows_error(pid: u32, operation: &'static str, error: winsafe::co::ERROR) -> Box<dyn Error> {
    if error == winsafe::co::ERROR::ACCESS_DENIED {
        Box::new(AccessDenied { pid, operation })
    } else {
        match operation {
            "打开进程" => format!("无法打开进程 {pid}：{error}").into(),
            "读取创建时间" => format!("无法读取进程 {pid} 创建时间：{error}").into(),
            "读取映像" => format!("无法读取进程 {pid} 映像：{error}").into(),
            _ => Box::new(error),
        }
    }
}

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
    use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};

    let handle = match HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(co::ERROR::INVALID_PARAMETER) => return Ok(None),
        Err(error) => return Err(windows_error(pid, "打开进程", error)),
    };
    if handle
        .WaitForSingleObject(Some(0))
        .map_err(|error| windows_error(pid, "首次等待退出", error))?
        == co::WAIT::OBJECT_0
    {
        return Ok(None);
    }
    let observation = windows_snapshot(&handle, pid);
    // 退出期间映像查询可能先失败，必须由同一已持有句柄证明退出，不能吞掉权限错误。
    let timeout = if observation.is_err() { 1000 } else { 0 };
    confirm_live_observation(
        observation,
        handle
            .WaitForSingleObject(Some(timeout))
            .map(|status| status == co::WAIT::OBJECT_0)
            .map_err(|error| windows_error(pid, "确认等待退出", error)),
    )
}

#[cfg(windows)]
pub(crate) fn windows_snapshot(handle: &winsafe::HPROCESS, pid: u32) -> Result<Value> {
    use winsafe::{co, prelude::kernel_Hprocess};

    let (started, _) = windows_lifetime(handle, pid)?;
    let executable = handle
        .QueryFullProcessImageName(co::PROCESS_NAME::WIN32)
        .map_err(|error| windows_error(pid, "读取映像", error))?;
    let executable = executable
        .strip_prefix(r"\\?\UNC\")
        .map(|value| format!(r"\\{value}"))
        .or_else(|| executable.strip_prefix(r"\\?\").map(ToOwned::to_owned))
        .unwrap_or(executable);
    Ok(json!({
        "pid": pid,
        "started": started.to_string(),
        "executable": executable.replace('/', "\\"),
    }))
}

#[cfg(windows)]
pub(crate) fn windows_lifetime(handle: &winsafe::HPROCESS, pid: u32) -> Result<(u64, u64)> {
    use winsafe::{FILETIME, prelude::kernel_Hprocess};

    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    handle
        .GetProcessTimes(&mut creation, &mut exit, &mut kernel, &mut user)
        .map_err(|error| windows_error(pid, "读取创建时间", error))?;
    let ticks =
        |value: FILETIME| (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime);
    Ok((ticks(creation), ticks(exit)))
}

#[cfg(windows)]
pub(crate) fn confirm_live_observation<T>(
    observation: Result<T>,
    exited: Result<bool>,
) -> Result<Option<T>> {
    match (observation, exited) {
        (_, Ok(true)) => Ok(None),
        (value, Ok(false)) => value.map(Some),
        (Ok(_), Err(error)) => Err(error),
        (Err(observation), Err(wait)) => Err(Box::new(ObservationFailure { observation, wait })),
    }
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
            let stat_exists = root
                .join("stat")
                .try_exists()
                .map_err(|error| format!("无法复核进程 {pid} 状态是否存在：{error}"))?;
            if !stat_exists {
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

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/lifecycle/identity.rs"]
mod tests;
