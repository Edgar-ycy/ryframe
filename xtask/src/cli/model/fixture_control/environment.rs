use std::path::PathBuf;

use super::{FixtureFileOutput, FixtureSide};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureEnvironmentInputs {
    pub(crate) review: PathBuf,
    pub(crate) fixture: PathBuf,
    pub(crate) maintenance_build: PathBuf,
    pub(crate) side: FixtureSide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureEnvironmentCommand {
    Help,
    Plan(FixtureEnvironmentInputs),
    Prepare {
        inputs: FixtureEnvironmentInputs,
        output: PathBuf,
        secrets_dir: Option<PathBuf>,
    },
    Review {
        review: PathBuf,
        output: FixtureFileOutput,
    },
    RotateSecrets {
        fixture: PathBuf,
        output: PathBuf,
    },
    BootstrapSecrets {
        source_fixture: PathBuf,
        source_secrets: PathBuf,
        fixture: PathBuf,
    },
}

impl FixtureEnvironmentCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Plan(_) => "plan",
            Self::Prepare { .. } => "prepare",
            Self::Review { .. } => "review",
            Self::RotateSecrets { .. } => "rotate-secrets",
            Self::BootstrapSecrets { .. } => "bootstrap-secrets",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        !matches!(self, Self::Help | Self::Plan(_))
    }
}
