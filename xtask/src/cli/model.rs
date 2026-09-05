use std::{fmt, path::PathBuf};

use crate::devex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cli {
    pub(crate) frontend_dir: PathBuf,
    pub(crate) command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Command {
    Dev { measure_once: bool },
    Check(CheckCommand),
    Build(BuildOptions),
    Generate(GenerateCommand),
    Data(DataCommand),
    Help(Option<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckCommand {
    Run(CheckOptions),
    Doctor,
    Ci(CiCommand),
    Perf(devex::DevexCommand),
    Release(ReleaseOptions),
    Recovery(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CheckOptions {
    pub(crate) scope: CheckScope,
    pub(crate) full: bool,
    pub(crate) plan: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BuildOptions {
    pub(crate) profile: BuildProfile,
    pub(crate) real: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildProfile {
    Release,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GenerateCommand {
    Help,
    Resource(ResourceCommand),
    Api(ApiGenerateCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ApiGenerateCommand {
    pub(crate) reference: Option<String>,
    pub(crate) write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DataCommand {
    Help,
    Migrate(MigrationCommand),
    Backup(Vec<String>),
    Restore(Vec<String>),
    TargetInventory(Vec<String>),
    File(Vec<String>),
    Reset(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CiCommand {
    Plan,
    Preflight,
    RustGate,
    ResourceGate,
    Integration,
    ConsumerContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckScope {
    All,
    Backend,
    Frontend,
}

impl CheckScope {
    pub(super) fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "all" => Ok(Self::All),
            "backend" => Ok(Self::Backend),
            "frontend" => Ok(Self::Frontend),
            _ => Err(CliError::new("--scope 只允许 all、backend 或 frontend")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReleaseOptions {
    pub(crate) tag: String,
    pub(crate) backend_repository: String,
    pub(crate) backend_commit: String,
    pub(crate) frontend_repository: String,
    pub(crate) frontend_commit: String,
    pub(crate) manifest_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceAction {
    Preview,
    Check,
    Write,
    Explain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResourceTarget {
    Named(String),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceCommand {
    pub(crate) target: ResourceTarget,
    pub(crate) action: ResourceAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationOperation {
    Verify,
    Up,
    Status,
}

impl MigrationOperation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Verify => "verify",
            Self::Up => "up",
            Self::Status => "status",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MigrationTarget {
    Control,
    TenantDataAll,
    TenantDataOne(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationScope {
    Control,
    TenantData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MigrationCommand {
    Freeze,
    Baseline,
    Run {
        operation: MigrationOperation,
        target: MigrationTarget,
    },
    New {
        scope: MigrationScope,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliError {
    message: String,
}

impl CliError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}
