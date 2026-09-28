use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixtureServiceOperation {
    Rustfs,
    Redis,
    Buckets,
    Status,
    Close,
    Reconcile,
    Recover,
    Restart,
}

impl FixtureServiceOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Rustfs => "rustfs",
            Self::Redis => "redis",
            Self::Buckets => "buckets",
            Self::Status => "status",
            Self::Close => "close",
            Self::Reconcile => "reconcile",
            Self::Recover => "recover",
            Self::Restart => "restart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureServicesCommand {
    Help,
    Run(FixtureServicesOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureServicesOptions {
    pub(crate) operation: FixtureServiceOperation,
    pub(crate) review: PathBuf,
    pub(crate) environment: PathBuf,
    pub(crate) owner_binding: Option<PathBuf>,
}

impl FixtureServicesCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Run(options) => options.operation.as_str(),
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(self, Self::Run(options) if !matches!(options.operation, FixtureServiceOperation::Status))
    }
}
