use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureReviewCommand {
    Help,
    Renew(FixtureReviewOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureReviewOptions {
    pub(crate) template: PathBuf,
    pub(crate) fixture: PathBuf,
    pub(crate) future_root: PathBuf,
    pub(crate) id: String,
    pub(crate) api_port: u16,
    pub(crate) worker_port: u16,
    pub(crate) frontend_port: u16,
    pub(crate) rustfs_api_port: u16,
    pub(crate) rustfs_console_port: u16,
    pub(crate) redis_port: u16,
    pub(crate) output: PathBuf,
}

impl FixtureReviewCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Renew(_) => "renew",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(self, Self::Renew(_))
    }
}
