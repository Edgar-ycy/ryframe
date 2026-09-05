use std::{
    ops::{Deref, DerefMut},
    process::{Child, Command},
};

use crate::Result;

use super::prepare_process_group;

/// 开发任务的子进程工厂。每次 spawn 都创建独立的可终止进程树，避免一个构建代次
/// 借用整个 dev 会话的生命周期。
pub(crate) struct ChildGroup;

/// 一个直接子进程及其独立进程树所有权。
pub(crate) struct ManagedChild {
    child: Child,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(unix)]
    process_group_id: u32,
}

impl ChildGroup {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self)
    }

    pub(crate) fn spawn(&self, command: &mut Command) -> Result<ManagedChild> {
        prepare_process_group(command);
        #[cfg(windows)]
        let job = create_kill_on_close_job()?;
        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                #[cfg(windows)]
                unsafe {
                    windows_sys::Win32::Foundation::CloseHandle(job);
                }
                return Err(error.into());
            }
        };
        #[cfg(unix)]
        let process_group_id = child.id();
        #[cfg(windows)]
        let mut child = child;
        #[cfg(windows)]
        {
            if let Err(error) = assign_and_resume_suspended_child(job, &child) {
                let _ = child.kill();
                let _ = child.wait();
                unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
                return Err(error);
            }
        }
        Ok(ManagedChild {
            child,
            #[cfg(windows)]
            job,
            #[cfg(unix)]
            process_group_id,
        })
    }
}

impl Deref for ManagedChild {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl DerefMut for ManagedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl ManagedChild {
    #[cfg(windows)]
    pub(crate) fn active_process_count(&self) -> Result<Option<u32>> {
        use std::{ffi::c_void, mem};
        use windows_sys::Win32::System::JobObjects::{
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
            QueryInformationJobObject,
        };

        let mut information = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let queried = unsafe {
            QueryInformationJobObject(
                self.job,
                JobObjectBasicAccountingInformation,
                (&raw mut information).cast::<c_void>(),
                u32::try_from(mem::size_of_val(&information))
                    .expect("Job Object 统计大小必须可由 u32 表示"),
                std::ptr::null_mut(),
            )
        };
        if queried == 0 {
            return Err(format!(
                "无法查询 Windows 子进程 Job Object：{}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        Ok(Some(information.ActiveProcesses))
    }

    #[cfg(not(windows))]
    pub(crate) const fn active_process_count(&self) -> Result<Option<u32>> {
        Ok(None)
    }

    #[cfg(windows)]
    pub(super) fn terminate_tree(&self) -> Result<()> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        if unsafe { TerminateJobObject(self.job, 1) } == 0 {
            return Err(format!(
                "无法终止 Windows 子进程 Job Object：{}",
                std::io::Error::last_os_error()
            )
            .into());
        }
        Ok(())
    }

    #[cfg(unix)]
    pub(super) const fn process_group_id(&self) -> u32 {
        self.process_group_id
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if !self.job.is_null() {
                unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
                self.job = std::ptr::null_mut();
            }
        }
        #[cfg(unix)]
        {
            let _ = super::signal_process_group(
                self.process_group_id,
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[cfg(windows)]
fn create_kill_on_close_job() -> Result<windows_sys::Win32::Foundation::HANDLE> {
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
    let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
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
    Ok(job)
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
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
    };
    use winsafe::{HPROCESSLIST, co::TH32CS, prelude::*};

    let mut snapshot = HPROCESSLIST::CreateToolhelp32Snapshot(TH32CS::SNAPTHREAD, None)
        .map_err(|error| format!("无法枚举挂起子进程的初始线程：{error}"))?;
    let mut last_open_error = None;
    for entry in snapshot.iter_threads() {
        let entry = entry.map_err(|error| format!("无法枚举挂起子进程的初始线程：{error}"))?;
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
    }
    if let Some(error) = last_open_error {
        Err(format!("无法打开挂起子进程 {process_id} 的初始线程：{error}").into())
    } else {
        Err(format!("未找到挂起子进程 {process_id} 的初始线程").into())
    }
}
