use std::path::PathBuf;

use super::{FixtureFileOutput, FixtureSide};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureRequestCommand {
    Help,
    Publish(FixtureRequestOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureRequestOptions {
    pub(crate) environment: PathBuf,
    pub(crate) service_run: PathBuf,
    pub(crate) id: String,
    pub(crate) side: FixtureSide,
    pub(crate) output: FixtureFileOutput,
}

impl FixtureRequestCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Publish(_) => "publish",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(self, Self::Publish(_))
    }
}
