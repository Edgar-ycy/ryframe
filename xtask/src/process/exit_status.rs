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

    pub(crate) fn append_context(&mut self, message: &str) {
        self.message.push_str("；后续任务报告：");
        self.message.push_str(message);
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

/// 只发布已知静态类别；子进程原文可能含凭据或 URL userinfo。
pub(crate) fn command_output_diagnostic(stdout: &[u8], stderr: &[u8]) -> &'static str {
    let codes = [stdout, stderr].map(String::from_utf8_lossy);
    let has_code = |expected| {
        codes.iter().any(|text| {
            text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .any(|code| code == expected)
        })
    };
    match (
        has_code("ERR_PNPM_NO_OFFLINE_META"),
        has_code("ERR_PNPM_META_FETCH_FAIL"),
    ) {
        (true, true) => {
            "；ERR_PNPM_NO_OFFLINE_META / ERR_PNPM_META_FETCH_FAIL：核对 verification.pnpm_cache_dir；在独立目录使用同一 registry 联网准备所需缓存/元数据和冻结依赖并通过策略检查，保留失败现场后使用新输出目录；不得关闭供应链校验或直接重放"
        }
        (true, false) => {
            "；ERR_PNPM_NO_OFFLINE_META：核对 verification.pnpm_cache_dir；在独立目录使用同一 registry 联网准备所需缓存/元数据和冻结依赖并通过策略检查，保留失败现场后使用新输出目录；不得关闭供应链校验或直接重放"
        }
        (false, true) => {
            "；ERR_PNPM_META_FETCH_FAIL：核对 verification.pnpm_cache_dir 及同一 registry 的所需缓存/元数据、冻结依赖及策略检查；保留失败现场，不自动重试，不关闭供应链校验"
        }
        _ if codes.iter().any(|text| {
            text.contains("fatal error C1083") && text.contains("Invalid argument")
        }) =>
        {
            "；已确认 MSVC C1083 报告无效输出路径；检查路径长度和访问权限，在工具文件中为 generator.target_dir 配置较短的固定目录，修复后先预览；未回显子进程原文"
        }
        _ => "",
    }
}
