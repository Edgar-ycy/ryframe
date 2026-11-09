//! 树控制独享的同句柄观察；两个字段共享截止时间，不改变全局身份查询预算。
use crate::Result;
use serde_json::Value;
use std::{io, time::Duration};

#[cfg(windows)]
pub(super) fn open(
    pid: u32,
    terminate: bool,
) -> Result<Option<winsafe::guard::CloseHandleGuard<winsafe::HPROCESS>>> {
    use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};
    let mut access = co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE;
    if terminate {
        access |= co::PROCESS::TERMINATE;
    }
    match HPROCESS::OpenProcess(access, false, pid) {
        Ok(handle) => Ok(Some(handle)),
        Err(co::ERROR::INVALID_PARAMETER) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(windows)]
pub(super) fn observe(pid: u32) -> Result<Option<Value>> {
    match open(pid, false)? {
        Some(handle) => snapshot(&handle, pid),
        None => Ok(None),
    }
}

#[cfg(windows)]
pub(super) fn snapshot(handle: &winsafe::HPROCESS, pid: u32) -> Result<Option<Value>> {
    use winsafe::{FILETIME, co, prelude::kernel_Hprocess};
    let started = std::time::Instant::now();
    let deadline = Duration::from_secs(5);
    let exited =
        || -> Result<bool> { Ok(handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0) };
    let now = || started.elapsed();
    let creation = query_with(
        || {
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            handle
                .GetProcessTimes(&mut creation, &mut exit, &mut kernel, &mut user)
                .map_err(query_error)?;
            Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
        },
        exited,
        &now,
        std::thread::sleep,
        deadline,
    )?;
    let Some(creation) = creation else {
        return Ok(None);
    };
    let image = query_with(
        || {
            handle
                .QueryFullProcessImageName(co::PROCESS_NAME::WIN32)
                .map_err(query_error)
        },
        exited,
        &now,
        std::thread::sleep,
        deadline,
    )?;
    let Some(image) = image else {
        return Ok(None);
    };
    if exited()? {
        return Ok(None);
    }
    let image = image
        .strip_prefix(r"\\?\UNC\")
        .map(|value| format!(r"\\{value}"))
        .or_else(|| image.strip_prefix(r"\\?\").map(ToOwned::to_owned))
        .unwrap_or(image);
    Ok(Some(
        serde_json::json!({"pid":pid,"started":creation.to_string(),"executable":image.replace('/', "\\")}),
    ))
}

#[cfg(windows)]
fn query_error(error: winsafe::co::ERROR) -> io::Error {
    match error {
        winsafe::co::ERROR::ACCESS_DENIED => io::Error::from_raw_os_error(5),
        winsafe::co::ERROR::GEN_FAILURE => io::Error::from_raw_os_error(31),
        _ => io::Error::other(error),
    }
}

fn query_with<T>(
    mut query: impl FnMut() -> io::Result<T>,
    mut exited: impl FnMut() -> Result<bool>,
    now: &impl Fn() -> Duration,
    mut pause: impl FnMut(Duration),
    deadline: Duration,
) -> Result<Option<T>> {
    loop {
        if exited()? {
            return Ok(None);
        }
        match query() {
            Ok(value) => return Ok(Some(value)),
            Err(error) => {
                if exited()? {
                    return Ok(None);
                }
                let remaining = deadline.saturating_sub(now());
                if !matches!(error.raw_os_error(), Some(5 | 31)) || remaining.is_zero() {
                    return Err(error.into());
                }
                pause(Duration::from_millis(50).min(remaining));
                if now() >= deadline {
                    if exited()? {
                        return Ok(None);
                    }
                    return Err(error.into());
                }
            }
        }
    }
}

pub(super) fn control_with(
    expected: &Value,
    observe: impl FnOnce() -> Result<Option<Value>>,
    terminate: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if let Some(actual) = observe()? {
        super::check(actual == *expected, "停止句柄的创建身份或映像不同")?;
        terminate()?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/fixture_control/services/lifecycle/tree_windows.rs"]
mod tests;
