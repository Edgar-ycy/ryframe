use std::path::PathBuf;

pub(crate) const CLONE_USAGE: &str = "用法：cargo xtask check recovery clone <plan|verify|init|status|stage|runtime|recover|recover-copy|bridge|post-copy|seed-runtime|storage|cache|maintenance> ...";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneEffect {
    ReadOnly,
    EvidenceWrite,
    BusinessWrite,
}

impl CloneEffect {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::EvidenceWrite => "evidence-write",
            Self::BusinessWrite => "business-write",
        }
    }

    pub(crate) const fn requires_write(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneStage {
    Export(CloneStageMode),
    TargetVerify,
    Copy(CloneStageMode),
}

impl CloneStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Export(_) => "export",
            Self::TargetVerify => "target-verify",
            Self::Copy(_) => "copy",
        }
    }

    pub(crate) const fn mode(self) -> CloneStageMode {
        match self {
            Self::Export(mode) | Self::Copy(mode) => mode,
            Self::TargetVerify => CloneStageMode::Run,
        }
    }

    pub(crate) const fn effect(self) -> CloneEffect {
        match self {
            Self::Copy(CloneStageMode::Run | CloneStageMode::Resume) => CloneEffect::BusinessWrite,
            Self::Copy(CloneStageMode::Reconcile) => CloneEffect::EvidenceWrite,
            Self::Export(_) | Self::TargetVerify => CloneEffect::EvidenceWrite,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneStageMode {
    Run,
    Reconcile,
    Resume,
}

impl CloneStageMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Reconcile => "reconcile",
            Self::Resume => "resume",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneSide {
    Source,
    Target,
}

impl CloneSide {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Target => "target",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneRole {
    Api,
    Worker,
}

impl CloneRole {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Worker => "worker",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneRuntimeOperation {
    Start,
    Stop,
    Status,
    Recover,
}

impl CloneRuntimeOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Status => "status",
            Self::Recover => "recover",
        }
    }

    pub(crate) const fn effect(self) -> CloneEffect {
        match self {
            Self::Status => CloneEffect::ReadOnly,
            Self::Recover => CloneEffect::EvidenceWrite,
            Self::Start | Self::Stop => CloneEffect::BusinessWrite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloneRuntimeOptions {
    pub(crate) run_dir: PathBuf,
    pub(crate) side: CloneSide,
    pub(crate) operation: CloneRuntimeOperation,
    pub(crate) roles: Vec<CloneRole>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PostCopyOperation {
    Register,
    Amend,
    Prepare,
    Schedules,
    Reconcile,
    Verify,
    RecoverSession,
}

impl PostCopyOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Amend => "amend",
            Self::Prepare => "prepare",
            Self::Schedules => "schedules",
            Self::Reconcile => "reconcile",
            Self::Verify => "verify",
            Self::RecoverSession => "recover-session",
        }
    }

    pub(crate) const fn effect(self) -> CloneEffect {
        match self {
            Self::Register | Self::Amend | Self::Prepare | Self::Reconcile | Self::Verify => {
                CloneEffect::EvidenceWrite
            }
            Self::Schedules | Self::RecoverSession => CloneEffect::BusinessWrite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PostCopyOptions {
    pub(crate) run_dir: PathBuf,
    pub(crate) operation: PostCopyOperation,
    pub(crate) request: Option<PathBuf>,
    pub(crate) producer_binding: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeedRuntimeOperation {
    Register,
    QuotasPlan,
    QuotasApply,
    QuotasReconcile,
    DepartmentsPlan,
    DepartmentsApply,
    DepartmentsReconcile,
    DepartmentsVerify,
    IdentitiesApply,
    IdentitiesVerify,
    Prepare,
    Start,
    Close,
    ArmInput,
    Stop,
    Status,
    Recover,
    RecoverSession,
}

impl SeedRuntimeOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::QuotasPlan => "quotas-plan",
            Self::QuotasApply => "quotas-apply",
            Self::QuotasReconcile => "quotas-reconcile",
            Self::DepartmentsPlan => "departments-plan",
            Self::DepartmentsApply => "departments-apply",
            Self::DepartmentsReconcile => "departments-reconcile",
            Self::DepartmentsVerify => "departments-verify",
            Self::IdentitiesApply => "identities-apply",
            Self::IdentitiesVerify => "identities-verify",
            Self::Prepare => "prepare",
            Self::Start => "start",
            Self::Close => "close",
            Self::ArmInput => "arm-input",
            Self::Stop => "stop",
            Self::Status => "status",
            Self::Recover => "recover",
            Self::RecoverSession => "recover-session",
        }
    }

    pub(crate) const fn effect(self) -> CloneEffect {
        match self {
            Self::Status => CloneEffect::ReadOnly,
            Self::Register
            | Self::QuotasPlan
            | Self::QuotasReconcile
            | Self::DepartmentsPlan
            | Self::DepartmentsReconcile
            | Self::DepartmentsVerify
            | Self::IdentitiesVerify
            | Self::Prepare
            | Self::ArmInput
            | Self::Recover => CloneEffect::EvidenceWrite,
            Self::QuotasApply
            | Self::DepartmentsApply
            | Self::IdentitiesApply
            | Self::Start
            | Self::Close
            | Self::Stop
            | Self::RecoverSession => CloneEffect::BusinessWrite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeedRuntimeOptions {
    pub(crate) run_dir: PathBuf,
    pub(crate) operation: SeedRuntimeOperation,
    pub(crate) request: Option<PathBuf>,
    pub(crate) producer_binding: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageOperation {
    Restart,
    Status,
    Stop,
    Recover,
}

impl StorageOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Status => "status",
            Self::Stop => "stop",
            Self::Recover => "recover",
        }
    }

    pub(crate) const fn effect(self) -> CloneEffect {
        match self {
            Self::Status => CloneEffect::ReadOnly,
            Self::Restart | Self::Stop | Self::Recover => CloneEffect::BusinessWrite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StorageOptions {
    pub(crate) run_dir: PathBuf,
    pub(crate) side: CloneSide,
    pub(crate) operation: StorageOperation,
    pub(crate) request: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheOperation {
    Restart,
    Status,
    Stop,
    Recover,
    Reconcile,
    Resume,
}

impl CacheOperation {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::Status => "status",
            Self::Stop => "stop",
            Self::Recover => "recover",
            Self::Reconcile => "reconcile",
            Self::Resume => "resume",
        }
    }

    pub(crate) const fn effect(self) -> CloneEffect {
        match self {
            Self::Status => CloneEffect::ReadOnly,
            Self::Reconcile => CloneEffect::EvidenceWrite,
            Self::Restart | Self::Stop | Self::Recover | Self::Resume => CloneEffect::BusinessWrite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheOptions {
    pub(crate) run_dir: PathBuf,
    pub(crate) operation: CacheOperation,
    pub(crate) request: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneCommand {
    Help,
    Plan {
        input: PathBuf,
        output: PathBuf,
    },
    Verify {
        plan: PathBuf,
    },
    Init {
        manifest: PathBuf,
        run_dir: PathBuf,
    },
    Status {
        run_dir: PathBuf,
    },
    Stage {
        run_dir: PathBuf,
        stage: CloneStage,
    },
    Runtime(CloneRuntimeOptions),
    Recover {
        run_dir: PathBuf,
        owner_binding: PathBuf,
    },
    RecoverCopy {
        run_dir: PathBuf,
        owner_binding: PathBuf,
    },
    Bridge {
        build: PathBuf,
        inventory: PathBuf,
        output: PathBuf,
    },
    PostCopy(PostCopyOptions),
    SeedRuntime(SeedRuntimeOptions),
    Storage(StorageOptions),
    Cache(CacheOptions),
    MaintenanceBuild {
        output: PathBuf,
    },
    MaintenanceVerify {
        output: PathBuf,
    },
}

impl CloneCommand {
    pub(crate) const fn name(&self) -> &'static str {
        match self {
            Self::Help => "help",
            Self::Plan { .. } => "plan",
            Self::Verify { .. } => "verify",
            Self::Init { .. } => "init",
            Self::Status { .. } => "status",
            Self::Stage { .. } => "stage",
            Self::Runtime(_) => "runtime",
            Self::Recover { .. } => "recover",
            Self::RecoverCopy { .. } => "recover-copy",
            Self::Bridge { .. } => "bridge",
            Self::PostCopy(_) => "post-copy",
            Self::SeedRuntime(_) => "seed-runtime",
            Self::Storage(_) => "storage",
            Self::Cache(_) => "cache",
            Self::MaintenanceBuild { .. } | Self::MaintenanceVerify { .. } => "maintenance",
        }
    }

    pub(crate) const fn effect(&self) -> CloneEffect {
        match self {
            Self::Help
            | Self::Verify { .. }
            | Self::Status { .. }
            | Self::MaintenanceVerify { .. } => CloneEffect::ReadOnly,
            Self::Plan { .. }
            | Self::Init { .. }
            | Self::Recover { .. }
            | Self::RecoverCopy { .. }
            | Self::Bridge { .. }
            | Self::MaintenanceBuild { .. } => CloneEffect::EvidenceWrite,
            Self::Stage { stage, .. } => stage.effect(),
            Self::Runtime(options) => options.operation.effect(),
            Self::PostCopy(options) => options.operation.effect(),
            Self::SeedRuntime(options) => options.operation.effect(),
            Self::Storage(options) => options.operation.effect(),
            Self::Cache(options) => options.operation.effect(),
        }
    }
}
