use std::path::PathBuf;

/// 性能身份准备的公开请求；所有写入都已由解析器确认显式 `--write`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PerformanceIdentitiesCommand {
    Plan {
        environment: PathBuf,
        output: PathBuf,
    },
    Apply {
        plan: PathBuf,
        state_dir: PathBuf,
    },
    Verify {
        plan: PathBuf,
        state_dir: PathBuf,
    },
}

impl PerformanceIdentitiesCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Plan { .. } => "plan",
            Self::Apply { .. } => "apply",
            Self::Verify { .. } => "verify",
        }
    }
}
