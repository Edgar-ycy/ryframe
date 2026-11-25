use std::path::PathBuf;

pub(crate) const CLONE_USAGE: &str = "用法：cargo xtask check recovery clone <plan|verify|init|status|stage|runtime|recover|recover-copy|prepare-lock-observe|prepare-lock-observe-interrupted|prepare-lock-recover|bridge|post-copy|seed-runtime|storage|cache|maintenance> ...\n  plan --input <.local-tests 内清单> --output <.local-tests 内新计划> --write\n  verify --plan <.local-tests 内计划>\n  init --manifest <.local-tests 内清单> --run-dir <.local-tests 内新目录> --write\n  status --run-dir <.local-tests 内运行目录>\n  recover-copy --run-dir <.local-tests 内运行目录> --owner-binding <.local-tests 内复制持有人绑定> --write\n  prepare-lock-observe --run-dir <原运行目录> --owner-binding <原准备控制器收据> --output <新本地前置收据> --write\n  prepare-lock-observe-interrupted --run-dir <原运行目录> --owner-binding <已退出的原准备控制器收据> --output <新本地退出后观察收据> --write\n  prepare-lock-recover --run-dir <原运行目录> --request <前置收据> --write\n  post-copy --run-dir <原运行目录> --operation <prepare|schedules|verify> [--originals held] --write\n  post-copy --originals held 仅适用于 Windows 上的 source_to_seed 复制后阶段\n  seed-runtime --run-dir <原运行目录> --operation <quotas-plan|quotas-apply> [--originals held] --write\n  seed-runtime --originals held 仅适用于 Windows 上的配额规划和应用\n  stage --run-dir <.local-tests 内运行目录> --stage <export|target-verify|copy|target-cache-markers> --mode <run|reconcile|resume> --write\n  bridge --build <构建收据> --inventory <来源清单> --output <.local-tests 内新桥接文件> --write\n  maintenance --operation build --output <.local-tests 内新目录> --write\n  maintenance --operation verify --output <维护构建目录或 build.json>\n  storage --run-dir <.local-tests 内运行目录> --side <source|target> --operation <restart|status|stop|recover> [--request <.local-tests 内请求>] [--write]\n\nplan/verify、bridge、maintenance build/verify、两种复制阶段的 init/status/stage、死亡控制器 recover 和 storage 由 Rust 原生状态机执行。plan/verify、bridge 和 init/status 只读本地输入，不连接业务资源；export 和 target-verify 分别由原生 Rust 执行：export 读取已登记的数据库与对象存储并写出新导出证据，target-verify 只读核验目标数据库、对象与 Redis 并发布本地证据；storage 按显式操作读取或管理已登记的 RustFS 进程与端口。copy 由 Rust 执行资源操作，并持有锁、attempt、来源终检、失败证据与结果发布。cache 正式入口由 Rust 直接执行固定阶段、资源与进程端口；已登记 post-copy 的 target/start/API 固定交接由同一 Rust 进程持有目标 Generation，并复用原生监督启动；同一已登记API的status/stop/recover核对原生完整树、精确停止并回收原外层锁，未完成操作保留且不重放；post-copy register/prepare/amend由同一Rust正式阶段持有存储锁、真实Generation和复制账本保护，登记固定请求、只读准备双角色私有配置或以原生失败/关闭历史发布下一环境修订代次；recover-copy 复核原控制器退出、直接命令终态及实际锁归属，只回收该会话的本地锁，未知写入仍须显式 reconcile；post-copy 业务和 seed-runtime 操作由 Rust 执行。clone runtime 的旧单进程 start/stop 由 Rust 按登记的产品与创建身份执行，status 只读核对来源或未登记目标的原运行收据、创建身份和监听，recover 仅核对并回收原外层控制锁。maintenance build 只通过统一进程层以 Cargo 离线模式启动本地构建；bridge 严格校验既有构建收据、完整来源清单、构建产物与当前产品输入后 create-new 写出复用审计。storage 保留 RustFS 固定请求、数据目录身份、原始启动与进程身份、端口、日志、失败和未知收据；status 只读，restart/stop/recover 必须显式 --write。";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneEffect {
    ReadOnly,
    EvidenceWrite,
    BusinessWrite,
}

impl CloneEffect {
    pub(crate) const fn requires_write(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneStage {
    Export(CloneStageMode),
    TargetVerify,
    Copy(CloneStageMode),
    TargetCacheMarkers(CloneStageMode),
}

impl CloneStage {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Export(_) => "export",
            Self::TargetVerify => "target-verify",
            Self::Copy(_) => "copy",
            Self::TargetCacheMarkers(_) => "target-cache-markers",
        }
    }

    pub(crate) const fn mode(self) -> CloneStageMode {
        match self {
            Self::Export(mode) | Self::Copy(mode) | Self::TargetCacheMarkers(mode) => mode,
            Self::TargetVerify => CloneStageMode::Run,
        }
    }

    pub(crate) const fn effect(self) -> CloneEffect {
        match self {
            Self::Copy(CloneStageMode::Run | CloneStageMode::Resume) => CloneEffect::BusinessWrite,
            Self::TargetCacheMarkers(CloneStageMode::Run) => CloneEffect::BusinessWrite,
            Self::Copy(CloneStageMode::Reconcile) | Self::TargetCacheMarkers(_) => {
                CloneEffect::EvidenceWrite
            }
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
    pub(crate) fn supports_held_originals(mode: &str) -> bool {
        cfg!(windows) && matches!(mode, "prepare" | "schedules" | "verify")
    }

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
    pub(crate) hold_originals: bool,
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
    pub(crate) fn supports_held_originals(mode: &str) -> bool {
        cfg!(windows) && matches!(mode, "quotas-plan" | "quotas-apply")
    }

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
    pub(crate) hold_originals: bool,
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
    ObservePrepareTargetLock {
        run_dir: PathBuf,
        owner_binding: PathBuf,
        output: PathBuf,
        after_exit: bool,
    },
    RecoverPrepareTargetLock {
        run_dir: PathBuf,
        receipt: PathBuf,
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
