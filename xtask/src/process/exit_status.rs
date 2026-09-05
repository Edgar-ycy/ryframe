//! 保留子任务退出码，避免把参数错误与执行失败合并。

use std::{error::Error, fmt, process::ExitStatus};

#[derive(Debug)]
pub(super) struct CommandFailure {
    message: String,
    status: ExitStatus,
}

impl CommandFailure {
    pub(super) fn new(message: String, status: ExitStatus) -> Self {
        Self { message, status }
    }
}

impl fmt::Display for CommandFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}（{}）", self.message, self.status)
    }
}

impl Error for CommandFailure {}

pub(crate) fn failure_exit_code(error: &(dyn Error + 'static)) -> Option<i32> {
    let failure = error.downcast_ref::<CommandFailure>()?;
    if let Some(code) = failure.status.code() {
        return Some(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        failure.status.signal().map(|signal| 128 + signal)
    }
    #[cfg(not(unix))]
    {
        None
    }
}
