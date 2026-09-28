use std::process::{Command, ExitStatus};

use crate::Result;
#[cfg(not(target_os = "linux"))]
use crate::process::ChildGroup;

use super::MemoryReading;

#[cfg(target_os = "linux")]
pub(crate) fn execute(command: Command) -> Result<(ExitStatus, MemoryReading)> {
    super::linux::execute(command)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn execute(command: Command) -> Result<(ExitStatus, MemoryReading)> {
    let mut child = ChildGroup::new()?.spawn(command)?;
    let status = child.wait()?;
    #[cfg(windows)]
    let reading = MemoryReading::from_result((|| {
        if child.active_process_count()? != Some(0) {
            return Err("主命令退出后仍有后代运行，内存测量窗口不完整".into());
        }
        child
            .job_stats()?
            .peak_memory_bytes
            .ok_or_else(|| "Windows Job Object 未提供峰值提交内存".into())
    })());
    #[cfg(not(windows))]
    let reading = MemoryReading::unavailable("当前平台没有完整进程树内存采集器");
    Ok((status, reading))
}
