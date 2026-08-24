use std::process::{Child, Command};

use crate::Result;

use super::prepare_process_group;

/// 开发任务的子进程容器。Windows 使用带 `KILL_ON_JOB_CLOSE` 的 Job Object，
/// 即使 xtask 异常退出也会回收 API、Worker、Vite 及其后代进程。
pub(crate) struct ChildGroup {
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}

impl ChildGroup {
    pub(crate) fn new() -> Result<Self> {
        #[cfg(windows)]
        {
            use std::{ffi::c_void, mem};
            use windows_sys::Win32::{
                Foundation::{CloseHandle, HANDLE},
                System::JobObjects::{
                    CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                    SetInformationJobObject,
                },
            };

            let job: HANDLE = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() {
                return Err(format!(
                    "无法创建 Windows Job Object：{}",
                    std::io::Error::last_os_error()
                )
                .into());
            }
            let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
            information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let configured = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const information).cast::<c_void>(),
                    u32::try_from(mem::size_of_val(&information))
                        .expect("Job Object 配置大小必须可由 u32 表示"),
                )
            };
            if configured == 0 {
                let error = std::io::Error::last_os_error();
                unsafe { CloseHandle(job) };
                return Err(format!("无法配置 Windows Job Object：{error}").into());
            }
            Ok(Self { job })
        }
        #[cfg(not(windows))]
        {
            Ok(Self {})
        }
    }

    pub(crate) fn spawn(&self, command: &mut Command) -> Result<Child> {
        prepare_process_group(command);
        let child = command.spawn()?;
        #[cfg(windows)]
        let mut child = child;
        #[cfg(windows)]
        {
            if let Err(error) = assign_and_resume_suspended_child(self.job, &child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
        Ok(child)
    }
}

#[cfg(windows)]
fn assign_and_resume_suspended_child(
    job: windows_sys::Win32::Foundation::HANDLE,
    child: &Child,
) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

    // Child 保有 CreateProcess 返回的真实进程句柄，可避免按 PID 再次打开时的竞态。
    let process = child.as_raw_handle().cast();
    if unsafe { AssignProcessToJobObject(job, process) } == 0 {
        return Err(format!(
            "无法把挂起的子进程加入 Windows Job Object：{}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    resume_initial_thread(child.id())
}

#[cfg(windows)]
fn resume_initial_thread(process_id: u32) -> Result<()> {
    use std::mem;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "无法枚举挂起子进程的初始线程：{}",
            std::io::Error::last_os_error()
        )
        .into());
    }

    let result = (|| {
        let mut entry = THREADENTRY32 {
            dwSize: u32::try_from(mem::size_of::<THREADENTRY32>())
                .expect("Windows 线程条目大小必须可由 u32 表示"),
            ..Default::default()
        };
        let mut has_entry = unsafe { Thread32First(snapshot, &raw mut entry) } != 0;
        let mut last_open_error = None;
        while has_entry {
            if entry.th32OwnerProcessID == process_id {
                let thread: HANDLE =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    last_open_error = Some(std::io::Error::last_os_error());
                } else {
                    let previous_suspend_count = unsafe { ResumeThread(thread) };
                    unsafe { CloseHandle(thread) };
                    if previous_suspend_count == u32::MAX {
                        return Err(format!(
                            "无法恢复挂起子进程 {process_id} 的初始线程：{}",
                            std::io::Error::last_os_error()
                        )
                        .into());
                    }
                    if previous_suspend_count == 1 {
                        return Ok(());
                    }
                    return Err(format!(
                        "挂起子进程 {process_id} 的初始线程挂起计数异常：{previous_suspend_count}"
                    )
                    .into());
                }
            }
            has_entry = unsafe { Thread32Next(snapshot, &raw mut entry) } != 0;
        }
        if let Some(error) = last_open_error {
            Err(format!("无法打开挂起子进程 {process_id} 的初始线程：{error}").into())
        } else {
            Err(format!("未找到挂起子进程 {process_id} 的初始线程").into())
        }
    })();
    unsafe { CloseHandle(snapshot) };
    result
}

#[cfg(windows)]
impl Drop for ChildGroup {
    fn drop(&mut self) {
        if !self.job.is_null() {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
            self.job = std::ptr::null_mut();
        }
    }
}
