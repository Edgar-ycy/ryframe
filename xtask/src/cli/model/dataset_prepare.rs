use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DatasetPrepareCommand {
    Help,
    Run(DatasetPrepareOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatasetPrepareOptions {
    pub(crate) plan: PathBuf,
    pub(crate) mode: DatasetPrepareMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DatasetPrepareMode {
    Prepare {
        preflight: PathBuf,
    },
    VerifyExisting {
        dataset: PathBuf,
        side: RecoverySide,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoverySide {
    Source,
    Target,
}

impl RecoverySide {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Target => "target",
        }
    }
}
