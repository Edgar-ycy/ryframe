use std::path::PathBuf;

pub(crate) const FILE_APPLY_CONFIRMATION: &str = "APPLY-FILE-A-MAINTENANCE";
pub(crate) const FILE_DEFAULT_BATCH_SIZE: u64 = 100;
pub(crate) const FILE_MAX_BATCH_SIZE: u64 = 1_000;
pub(crate) const FILE_DEFAULT_START_AFTER: i64 = i64::MIN;

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
    Begin(RestoreBeginOptions),
    VerifyData(RestoreDataOptions),
    Verify(RestoreVerifyOptions),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreBeginOptions {
    pub(crate) plan: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) restore_config_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreDataOptions {
    pub(crate) id: String,
    pub(crate) backup_root: PathBuf,
    pub(crate) output: PathBuf,
    pub(crate) restore_config_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RestoreVerifyOptions {
    pub(crate) id: String,
    pub(crate) proof: PathBuf,
    pub(crate) tests_receipt: PathBuf,
    pub(crate) runtime_receipt: PathBuf,
    pub(crate) target_plan: PathBuf,
    pub(crate) runner_root: PathBuf,
    pub(crate) restore_config_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetInventoryOptions {
    pub(crate) target: String,
    pub(crate) output: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileMaintenanceOperation {
    BackfillSha256,
    DrainLegacyReservations,
}

impl FileMaintenanceOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::BackfillSha256 => "backfill-sha256",
            Self::DrainLegacyReservations => "drain-legacy-reservations",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileMaintenanceMode {
    DryRun,
    Apply,
}

impl FileMaintenanceMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry-run",
            Self::Apply => "apply",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileMaintenanceOptions {
    pub(crate) operation: FileMaintenanceOperation,
    pub(crate) mode: FileMaintenanceMode,
    pub(crate) database: String,
    pub(crate) batch_size: u64,
    pub(crate) start_after: i64,
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
