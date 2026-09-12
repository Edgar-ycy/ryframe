use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureRetentionOptions {
    pub(crate) runtime_dir: PathBuf,
    pub(crate) tenant: String,
    pub(crate) migration: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FixtureRetentionCommand {
    Help,
    Inspect(FixtureRetentionOptions),
    PlanHistory(FixtureRetentionOptions),
    HistoricalExpired {
        options: FixtureRetentionOptions,
        plan_sha256: String,
    },
    ExportBackup(FixtureRetentionOptions),
    VerifyCleaned(FixtureRetentionOptions),
}

impl FixtureRetentionCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Inspect(_) => "inspect",
            Self::PlanHistory(_) => "plan-history",
            Self::HistoricalExpired { .. } => "historical-expired",
            Self::ExportBackup(_) => "export-backup",
            Self::VerifyCleaned(_) => "verify-cleaned",
        }
    }

    pub(crate) const fn writes(&self) -> bool {
        matches!(self, Self::HistoricalExpired { .. } | Self::ExportBackup(_))
    }
}
