use std::path::PathBuf;

use super::FixtureSide;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureDatasetInputs {
    pub(crate) environment: PathBuf,
    pub(crate) runtime: PathBuf,
    pub(crate) side: FixtureSide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureDatasetCommand {
    Help,
    Plan {
        inputs: FixtureDatasetInputs,
        work_dir: PathBuf,
        output: PathBuf,
    },
    Prepare {
        inputs: FixtureDatasetInputs,
        plan: PathBuf,
    },
}

impl FixtureDatasetCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Plan { .. } => "plan",
            Self::Prepare { .. } => "prepare",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        !matches!(self, Self::Help)
    }
}
