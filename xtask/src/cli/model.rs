use std::{collections::BTreeMap, fmt, path::PathBuf};

use crate::devex;

#[path = "model/release.rs"]
mod release;
pub(crate) use release::*;
#[path = "model/frontend_source.rs"]
mod frontend_source;
pub(crate) use frontend_source::*;
#[path = "model/performance_identities.rs"]
mod performance_identities;
pub(crate) use performance_identities::*;
#[path = "model/fixture_runtime.rs"]
mod fixture_runtime;
pub(crate) use fixture_runtime::*;
#[path = "model/data.rs"]
mod data;
pub(crate) use data::*;
#[path = "model/dataset_prepare.rs"]
mod dataset_prepare;
pub(crate) use dataset_prepare::*;
#[path = "model/monitoring.rs"]
mod monitoring;
pub(crate) use monitoring::*;
#[path = "model/fixture_control.rs"]
mod fixture_control;
pub(crate) use fixture_control::*;
#[path = "model/fixture_prepare.rs"]
mod fixture_prepare;
pub(crate) use fixture_prepare::*;
#[path = "model/recovery_inputs.rs"]
mod recovery_inputs;
pub(crate) use recovery_inputs::*;
#[path = "model/recovery_reference.rs"]
mod recovery_reference;
pub(crate) use recovery_reference::*;

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
    Release(ReleaseCommand),
    Recovery(RecoveryCommand),
}

/// 恢复验收由一个公开入口按职责选择私有阶段程序。
///
/// 各阶段仍保留底层参数校验，xtask 负责固定当前检出的路径，避免维护文档暴露
/// 私有脚本作为用户入口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryCommand {
    Reference(RecoveryReferenceCommand),
    Inputs(RecoveryInputsCommand),
    Runtime(Vec<String>),
    Source(Vec<String>),
    Clone(Vec<String>),
    SeedSource(SeedSourceOptions),
    FreshTarget(FreshTargetCommand),
    Fixture(Vec<String>),
    FixturePrepare(FixturePrepareCommand),
    FixtureControl(Box<FixtureControlCommand>),
    FixtureRuntime(FixtureRuntimeCommand),
    FullStack(FullStackCommand),
    Monitoring(MonitoringCommand),
    DatasetPrepare(DatasetPrepareCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeedSourceOperation {
    Register,
    Rebind,
    GenerationStart,
    GenerationStop,
    GenerationStatus,
    GenerationRecover,
    Export,
    ExportReconcile,
}

impl SeedSourceOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "source-register",
            Self::Rebind => "source-rebind",
            Self::GenerationStart => "source-generation-start",
            Self::GenerationStop => "source-generation-stop",
            Self::GenerationStatus => "source-generation-status",
            Self::GenerationRecover => "source-generation-recover",
            Self::Export => "source-export",
            Self::ExportReconcile => "source-export-reconcile",
        }
    }

    pub(crate) const fn requires_request(self) -> bool {
        matches!(
            self,
            Self::Rebind | Self::GenerationStart | Self::GenerationStop | Self::GenerationRecover
        )
    }

    pub(crate) const fn is_read_only(self) -> bool {
        matches!(self, Self::GenerationStatus)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeedSourceOptions {
    pub(crate) operation: SeedSourceOperation,
    pub(crate) run_dir: PathBuf,
    pub(crate) request: Option<PathBuf>,
    pub(crate) write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FreshTargetCommand {
    Help,
    Run(FreshTargetOptions),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FreshTargetOperation {
    Prepare,
    ResumePrepare,
    Initialize,
    ResumeInitialize,
    ReconcilePreflight,
    Verify,
    Status,
}

impl FreshTargetOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::ResumePrepare => "resume-prepare",
            Self::Initialize => "initialize",
            Self::ResumeInitialize => "resume-initialize",
            Self::ReconcilePreflight => "reconcile-preflight",
            Self::Verify => "verify",
            Self::Status => "status",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshTargetOptions {
    pub(crate) operation: FreshTargetOperation,
    pub(crate) workspace: PathBuf,
    pub(crate) request: Option<PathBuf>,
    pub(crate) environment: Option<PathBuf>,
    pub(crate) storage_run: Option<PathBuf>,
    pub(crate) observation_dir: Option<PathBuf>,
    pub(crate) write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FullStackCommand {
    Prepare,
    RateLimit { environment_file: PathBuf },
    Start,
    Collect,
    Help,
    RateLimitHelp,
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
    pub(crate) plan: bool,
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
    PerformanceIdentities(PerformanceIdentitiesCommand),
    Backup(BackupCommand),
    Restore(RestoreCommand),
    TargetInventory(TargetInventoryOptions),
    File(FileMaintenanceOptions),
    Reset(ResetCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CiCommand {
    Plan,
    Preflight,
    RustGate,
    ResourceGate,
    ResourceGateReplay(ResourceGateReplayOptions),
    Integration,
    ConsumerContract,
    FrontendSource(FrontendSourceOptions),
    Required(RequiredOptions),
    Security(SecurityCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SecurityCommand {
    Source,
    Report(SecurityReportOptions),
    Deployment(DeploymentOptions),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecurityReportKind {
    CycloneDx,
    Trivy,
}

impl SecurityReportKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CycloneDx => "cyclonedx",
            Self::Trivy => "trivy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SecurityReportOptions {
    pub(crate) kind: SecurityReportKind,
    pub(crate) input: PathBuf,
    pub(crate) require_reproducible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeploymentPhase {
    Source,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeploymentOptions {
    pub(crate) phase: DeploymentPhase,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) github_output: Option<PathBuf>,
    pub(crate) image: Option<String>,
    pub(crate) expected_commit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequiredEvent {
    Push,
    PullRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequiredAction {
    Opened,
    Synchronize,
    Reopened,
    Edited,
}

impl RequiredAction {
    pub(super) fn parse(
        event: RequiredEvent,
        value: Option<&str>,
    ) -> Result<Option<Self>, CliError> {
        match (event, value.unwrap_or_default()) {
            (RequiredEvent::Push, "") => Ok(None),
            (RequiredEvent::Push, _) => Err(CliError::new("push 事件不得提供 --action")),
            (RequiredEvent::PullRequest, "opened") => Ok(Some(Self::Opened)),
            (RequiredEvent::PullRequest, "synchronize") => Ok(Some(Self::Synchronize)),
            (RequiredEvent::PullRequest, "reopened") => Ok(Some(Self::Reopened)),
            (RequiredEvent::PullRequest, "edited") => Ok(Some(Self::Edited)),
            (RequiredEvent::PullRequest, "") => {
                Err(CliError::new("pull_request 事件必须提供 --action"))
            }
            (RequiredEvent::PullRequest, _) => Err(CliError::new(
                "pull_request --action 只允许 opened、synchronize、reopened 或 edited",
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Synchronize => "synchronize",
            Self::Reopened => "reopened",
            Self::Edited => "edited",
        }
    }
}

impl RequiredEvent {
    pub(super) fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "push" => Ok(Self::Push),
            "pull_request" => Ok(Self::PullRequest),
            _ => Err(CliError::new("--event 只允许 push 或 pull_request")),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::PullRequest => "pull_request",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequiredJobResult {
    Success,
    Failure,
    Cancelled,
    Skipped,
}

impl RequiredJobResult {
    pub(super) fn parse(value: &str) -> Result<Self, CliError> {
        match value {
            "success" => Ok(Self::Success),
            "failure" => Ok(Self::Failure),
            "cancelled" => Ok(Self::Cancelled),
            "skipped" => Ok(Self::Skipped),
            _ => Err(CliError::new(format!(
                "required job 结果只允许 success、failure、cancelled 或 skipped，实际为 {value}"
            ))),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequiredNeed {
    pub(crate) result: RequiredJobResult,
    pub(crate) outputs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequiredOptions {
    pub(crate) event: RequiredEvent,
    pub(crate) action: Option<RequiredAction>,
    pub(crate) needs: BTreeMap<String, RequiredNeed>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceGateReplayOptions {
    pub(crate) manifest: PathBuf,
    pub(crate) work_dir: PathBuf,
    pub(crate) report: PathBuf,
    pub(crate) activation_gate: bool,
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
