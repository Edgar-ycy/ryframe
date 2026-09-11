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

/// 跨线程任务只能传递可安全拥有的数据；这里保留首个失败的原始文本和退出码，
/// 避免并行编排把参数错误等具体退出状态压成普通的 `1`。
#[derive(Debug)]
pub(crate) struct PreservedFailure {
    message: String,
    exit_code: Option<i32>,
}

impl PreservedFailure {
    pub(crate) fn new(message: String, exit_code: Option<i32>) -> Self {
        Self { message, exit_code }
    }
}

impl fmt::Display for PreservedFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for PreservedFailure {}

pub(crate) fn failure_exit_code(error: &(dyn Error + 'static)) -> Option<i32> {
    if let Some(failure) = error.downcast_ref::<PreservedFailure>() {
        return failure.exit_code;
    }
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
