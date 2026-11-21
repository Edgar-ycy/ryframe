use std::{error::Error, fmt, time::Instant};

use winsafe::{FILETIME, HPROCESS, co, guard::CloseHandleGuard, prelude::kernel_Hprocess};

use super::windows_command_line;
use crate::Result;

/// Job 计数可能先于进程句柄进入终止状态归零；保持真实创建身份的句柄直到回收完成。
pub(super) struct MemberHandles(Vec<MemberHandle>);

struct MemberHandle {
    handle: CloseHandleGuard<HPROCESS>,
    pid: u32,
    creation: u64,
}

impl MemberHandle {
    fn has_exited(&self) -> Result<bool> {
        self.wait_for_exit(0)
    }

    fn wait_for_exit(&self, milliseconds: u32) -> Result<bool> {
        wait_for_exit(&self.handle, self.pid, Some(self.creation), milliseconds)
    }
}

impl MemberHandles {
    pub(super) fn capture(group: &processkit::ProcessGroup) -> Result<Self> {
        let mut handles = Vec::new();
        for member in group.members_info()? {
            if let Some(handle) = open_live_member(&member)? {
                handles.push(handle);
            }
        }
        Ok(Self(handles))
    }

    pub(super) fn all_exited(&self) -> Result<bool> {
        for handle in &self.0 {
            if !handle.has_exited()? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

pub(super) fn has_compiling_rustc(
    group: &processkit::ProcessGroup,
    compiler: &same_file::Handle,
    deadline: Instant,
    is_compilation: impl Fn(&[String]) -> bool,
) -> Result<bool> {
    for member in group.members_info()? {
        if Instant::now() >= deadline {
            break;
        }
        if !member
            .exe_name()
            .is_some_and(|name| name.eq_ignore_ascii_case("rustc.exe"))
        {
            continue;
        }
        let Some(process) = open_live_member(&member)? else {
            continue;
        };
        let pid = process.pid;
        let Some(image) = member_image(group, &process, deadline)? else {
            continue;
        };
        // Rustup 的代理也叫 rustc.exe，以文件身份绑定实际工具链的编译器。
        let identity = same_file::Handle::from_path(image);
        if process.has_exited()? || &identity? != compiler {
            continue;
        }
        let observed = windows_command_line::arguments_before(pid, deadline);
        // CIM 只按 PID 查询，必须用查询前持有的同一进程句柄核对仍然存活。
        if process.has_exited()? {
            continue;
        }
        // 同一活进程暂未返回参数时，继续沿用原截止时间观察。
        let Some(arguments) = observed? else {
            continue;
        };
        if is_compilation(&arguments) && Instant::now() < deadline {
            return Ok(true);
        }
    }
    Ok(false)
}

fn member_image(
    group: &processkit::ProcessGroup,
    process: &MemberHandle,
    deadline: Instant,
) -> Result<Option<String>> {
    let mut last_denied = None;
    while Instant::now() < deadline {
        // 枚举与映像快照可能跨越 PID 复用，每次查询前复核原 Job 和同一进程句柄。
        if !group.members()?.contains(&process.pid) || process.has_exited()? {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            break;
        }
        let image = process
            .handle
            .QueryFullProcessImageName(co::PROCESS_NAME::WIN32);
        if process.has_exited()? {
            return Ok(None);
        }
        match image {
            Ok(image) => return Ok(Some(image)),
            Err(cause) => {
                let error = member_error(
                    "QueryFullProcessImageName",
                    process.pid,
                    Some(process.creation),
                    cause,
                );
                if cause != co::ERROR::ACCESS_DENIED {
                    return Err(error.into());
                }
                last_denied = Some(error);
            }
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        // 只在剩余预算内等待同一句柄，退出状态以该句柄为准。
        let wait_ms = remaining.as_millis().min(10) as u32;
        if process.wait_for_exit(wait_ms)? {
            return Ok(None);
        }
    }
    if let Some(error) = last_denied {
        return Err(error.into());
    }
    Ok(None)
}

fn open_live_member(member: &processkit::MemberInfo) -> Result<Option<MemberHandle>> {
    let pid = member.pid();
    let handle = match HPROCESS::OpenProcess(
        co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
        false,
        pid,
    ) {
        Ok(handle) => handle,
        Err(co::ERROR::INVALID_PARAMETER) => return Ok(None),
        Err(error) => return Err(member_error("OpenProcess", pid, None, error).into()),
    };
    if wait_for_exit(&handle, pid, None, 0)? {
        return Ok(None);
    }
    let creation = creation_identity(&handle, pid)?;
    match member.start_time() {
        Some(started) if started == creation => Ok(Some(MemberHandle {
            handle,
            pid,
            creation,
        })),
        // PID 已复用时不接管无关进程。
        Some(_) => Ok(None),
        None => Err("Windows Job 成员缺少创建身份，无法确认完整回收".into()),
    }
}

fn wait_for_exit(
    handle: &HPROCESS,
    pid: u32,
    creation: Option<u64>,
    milliseconds: u32,
) -> Result<bool> {
    let wait = handle
        .WaitForSingleObject(Some(milliseconds))
        .map_err(|error| member_error("WaitForSingleObject", pid, creation, error))?;
    Ok(wait == co::WAIT::OBJECT_0)
}

fn creation_identity(handle: &HPROCESS, pid: u32) -> Result<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    handle
        .GetProcessTimes(&mut creation, &mut exit, &mut kernel, &mut user)
        .map_err(|error| member_error("GetProcessTimes", pid, None, error))?;
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

#[derive(Debug)]
struct MemberError {
    operation: &'static str,
    pid: u32,
    creation: Option<u64>,
    cause: co::ERROR,
}

fn member_error(
    operation: &'static str,
    pid: u32,
    creation: Option<u64>,
    cause: co::ERROR,
) -> MemberError {
    MemberError {
        operation,
        pid,
        creation,
        cause,
    }
}

impl fmt::Display for MemberError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Windows Job 成员 {}（PID {}",
            self.operation, self.pid
        )?;
        if let Some(creation) = self.creation {
            write!(formatter, "，实际创建 FILETIME {creation}")?;
        } else {
            formatter.write_str("，实际创建身份未确认")?;
        }
        write!(formatter, "）失败：{}", self.cause)
    }
}

impl Error for MemberError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.cause)
    }
}
