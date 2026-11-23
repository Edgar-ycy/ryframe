use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackupCommand {
    Inventory(BackupInventoryOptions),
    Register(BackupRegisterOptions),
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupInventoryOptions {
    pub(crate) output: PathBuf,
    pub(crate) source_sha: String,
    pub(crate) observation: BackupObservation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackupObservation {
    QuiescedAt(String),
    ObservedAt(String),
}

impl BackupObservation {
    pub(crate) fn flag_and_value(&self) -> (&'static str, &str) {
        match self {
            Self::QuiescedAt(value) => ("--quiesced-at", value),
            Self::ObservedAt(value) => ("--observed-at", value),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackupRegisterOptions {
    pub(crate) manifest: PathBuf,
    pub(crate) backup_root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestoreCommand {
    Begin(Box<RestoreBeginOptions>),
    VerifyData(RestoreDataOptions),
    Verify(Box<RestoreVerifyOptions>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreBeginOptions {
    pub(crate) plan: PathBuf,
    pub(crate) backup_root: PathBuf,
    pub(crate) target_plan: PathBuf,
    pub(crate) runtime_registration: PathBuf,
    pub(crate) maintenance_root: PathBuf,
    pub(crate) maintenance_build: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) copy: Option<RestoreBeginCopy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreBeginCopy {
    pub(crate) publisher_plan: PathBuf,
    pub(crate) id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreDataOptions {
    pub(crate) id: String,
    pub(crate) backup_root: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) restore_config_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreVerifyOptions {
    pub(crate) id: String,
    pub(crate) proof: PathBuf,
    pub(crate) tests_receipt: PathBuf,
    pub(crate) runtime_receipt: PathBuf,
    pub(crate) target_plan: PathBuf,
    pub(crate) coordinator_root: PathBuf,
    pub(crate) coordinator_sha: String,
    pub(crate) runner_root: PathBuf,
    pub(crate) config_dir: Option<PathBuf>,
    pub(crate) restore_config_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetInventoryOptions {
    pub(crate) target: String,
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResetCommand {
    Plan,
    Execute(ResetExecuteOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResetExecuteOptions {
    pub(crate) plan_hash: String,
    pub(crate) confirmation: String,
}
