use super::FixtureFileOutput;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureSourcePairCommand {
    Help,
    Publish(FixtureFileOutput),
    PublishExecution { output_dir: PathBuf },
}

impl FixtureSourcePairCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Publish(_) => "publish",
            Self::PublishExecution { .. } => "publish-execution",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(self, Self::Publish(_) | Self::PublishExecution { .. })
    }
}
