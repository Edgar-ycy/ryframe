use std::path::PathBuf;

pub(crate) const CLONE_USAGE: &str = "用法：cargo xtask check recovery clone <plan|verify|init|status|stage|runtime|recover|recover-copy|bridge|post-copy|seed-runtime|storage|cache|maintenance> ...\n  plan --input <.local-tests 内清单> --output <.local-tests 内新计划> --write\n  verify --plan <.local-tests 内计划>\n  init --manifest <.local-tests 内清单> --run-dir <.local-tests 内新目录> --write\n  status --run-dir <.local-tests 内运行目录>\n  stage --run-dir <.local-tests 内运行目录> --stage <export|target-verify|copy> --mode <run|reconcile|resume> --write\n  bridge --build <构建收据> --inventory <来源清单> --output <.local-tests 内新桥接文件> --write\n  maintenance --operation build --output <.local-tests 内新目录> --write\n  maintenance --operation verify --output <维护构建目录或 build.json>\n  storage --run-dir <.local-tests 内运行目录> --side <source|target> --operation <restart|status|stop|recover> [--request <.local-tests 内请求>] [--write]\n\nplan/verify、bridge、maintenance build/verify、source_to_seed init/status/export stage 和 storage 由 Rust 原生执行，不启动 Python。plan/verify、bridge 和 source_to_seed init/status 只读本地输入，不连接业务资源；export 会读取已登记的数据库与对象存储并写出新导出证据，storage 会按显式操作读取或管理已登记的 RustFS 进程与端口。source_to_seed 的 target-verify 和 copy 仍使用单一私有 Python 子进程。maintenance build 只通过统一进程层以 Cargo 离线模式启动本地构建；bridge 严格校验既有构建收据、完整来源清单、构建产物与当前产品输入后 create-new 写出复用审计。storage 保留 RustFS 固定请求、数据目录身份、原始启动与进程身份、端口、日志、失败和未知收据；status 只读，restart/stop/recover 必须显式 --write。cache、seed_to_arm 的 init/status/stage 及其他 clone 资源阶段继续使用原状态机。";

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
