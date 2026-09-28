use winsafe::{FILETIME, HPROCESS, co, guard::CloseHandleGuard, prelude::kernel_Hprocess};

use crate::Result;

/// Job 计数可能先于进程句柄进入终止状态归零；保持真实创建身份的句柄直到回收完成。
pub(super) struct MemberHandles(Vec<CloseHandleGuard<HPROCESS>>);

impl MemberHandles {
    pub(super) fn capture(group: &processkit::ProcessGroup) -> Result<Self> {
        let mut handles = Vec::new();
        for member in group.members_info()? {
            let handle = match HPROCESS::OpenProcess(
                co::PROCESS::QUERY_LIMITED_INFORMATION | co::PROCESS::SYNCHRONIZE,
                false,
                member.pid(),
            ) {
                Ok(handle) => handle,
                Err(co::ERROR::INVALID_PARAMETER) => continue,
                Err(error) => return Err(error.into()),
            };
            if handle.WaitForSingleObject(Some(0))? == co::WAIT::OBJECT_0 {
                continue;
            }
            let identity = creation_identity(&handle)?;
            if member.start_time() == Some(identity) {
                handles.push(handle);
            } else if member.start_time().is_none() {
                return Err("Windows Job 成员缺少创建身份，无法确认完整回收".into());
            }
            // PID 已复用时不接管无关进程；后续查询仍以原 Job 为资源边界。
        }
        Ok(Self(handles))
    }

    pub(super) fn all_exited(&self) -> Result<bool> {
        for handle in &self.0 {
            if handle.WaitForSingleObject(Some(0))? != co::WAIT::OBJECT_0 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn creation_identity(handle: &HPROCESS) -> Result<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    handle.GetProcessTimes(&mut creation, &mut exit, &mut kernel, &mut user)?;
    Ok((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}
