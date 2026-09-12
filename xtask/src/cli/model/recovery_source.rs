use std::path::{Path, PathBuf};

pub(crate) const SOURCE_USAGE: &str = "用法：cargo xtask check recovery source verify --source-generation <同代启动收据> --output <新验证收据> --write\n  cargo xtask check recovery source comparison-capture --b0-backend <绝对目录> --b0-adapter-backend <绝对目录> --b0-frontend <绝对目录> --b0-backend-build <收据> --b0-frontend-build <收据> --b1-backend <绝对目录> --b1-frontend <绝对目录> --b1-backend-build <收据> --b1-frontend-build <收据> --source-export-result <导出结果> --output <新来源清单> --write\n  cargo xtask check recovery source comparison-verify --receipt <来源清单>";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceOperation {
    Verify,
    ComparisonCapture,
    ComparisonVerify,
}

impl SourceOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Verify => "verify",
            Self::ComparisonCapture => "comparison-capture",
            Self::ComparisonVerify => "comparison-verify",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceVerifyOptions {
    pub(crate) source_generation: PathBuf,
    pub(crate) output: PathBuf,
}

pub(crate) fn is_source_runtime_output(output: &Path) -> bool {
    output
        .file_name()
        .is_some_and(|name| name == "source-runtime.json")
        && output
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "verification")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceComparisonCaptureOptions {
    pub(crate) b0_backend: PathBuf,
    pub(crate) b0_adapter_backend: PathBuf,
    pub(crate) b0_frontend: PathBuf,
    pub(crate) b0_backend_build: PathBuf,
    pub(crate) b0_frontend_build: PathBuf,
    pub(crate) b1_backend: PathBuf,
    pub(crate) b1_frontend: PathBuf,
    pub(crate) b1_backend_build: PathBuf,
    pub(crate) b1_frontend_build: PathBuf,
    pub(crate) source_export_result: PathBuf,
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SourceCommand {
    Help(Option<SourceOperation>),
    Verify(SourceVerifyOptions),
    ComparisonCapture(Box<SourceComparisonCaptureOptions>),
    ComparisonVerify { receipt: PathBuf },
}

impl SourceCommand {
    pub(crate) const fn operation(&self) -> Option<SourceOperation> {
        match self {
            Self::Help(operation) => *operation,
            Self::Verify(_) => Some(SourceOperation::Verify),
            Self::ComparisonCapture(_) => Some(SourceOperation::ComparisonCapture),
            Self::ComparisonVerify { .. } => Some(SourceOperation::ComparisonVerify),
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(
            Self::operation(self),
            Some(SourceOperation::Verify | SourceOperation::ComparisonCapture)
        )
    }
}
