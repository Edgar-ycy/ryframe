use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureArtifactOptions {
    pub(crate) runtime_dir: PathBuf,
    pub(crate) job_id: i64,
    pub(crate) receipt: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureArtifactCommand {
    Help,
    Snapshot(FixtureArtifactOptions),
    VerifyDeleted(FixtureArtifactOptions),
}

impl FixtureArtifactCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Snapshot(_) => "snapshot",
            Self::VerifyDeleted(_) => "verify-deleted",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(self, Self::Snapshot(_))
    }
}
