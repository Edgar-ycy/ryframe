#[cfg(windows)]
pub(crate) fn process_is_running(pid: u32) -> bool {
    use winsafe::{HPROCESS, co, prelude::kernel_Hprocess};

    let Ok(process) = HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) else {
        return false;
    };
    process
        .WaitForSingleObject(Some(0))
        .is_ok_and(|status| status == co::WAIT::TIMEOUT)
}

#[cfg(unix)]
pub(crate) fn process_is_running(pid: u32) -> bool {
    use nix::{errno::Errno, sys::signal::kill, unistd::Pid};

    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    matches!(kill(Pid::from_raw(pid), None), Ok(()) | Err(Errno::EPERM))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn process_is_running(pid: u32) -> bool {
    pid == std::process::id()
}
