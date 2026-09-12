use super::policy_tasks::STRICT_MIGRATION_HISTORY_ARGS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskRepository {
    Backend,
    Frontend,
    CrossRepository,
}

impl TaskRepository {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Backend => "后端",
            Self::Frontend => "前端",
            Self::CrossRepository => "跨仓",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskStage {
    Prerequisite,
    Static,
    Test,
    Contract,
    Build,
}

impl TaskStage {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Prerequisite => "前置",
            Self::Static => "静态",
            Self::Test => "测试",
            Self::Contract => "契约",
            Self::Build => "构建",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskWorkingDirectory {
    Backend,
    Frontend,
}

impl TaskWorkingDirectory {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Backend => "后端工作区",
            Self::Frontend => "前端工作区",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum TaskExecutor {
    RemovedIdentity,
    FrontendDependencies,
    PythonEnvironment,
    CargoFormat,
    FeatureRegistry,
    WorkspaceClippy,
    PolicyChecks,
    SnapshotPrepare,
    PythonTests,
    MigrationHistory,
    WorkspaceGates,
    SnapshotVerify,
    ConsumerContract,
    FrontendConsumerContract,
    FrontendFull,
    SmartBackendPackages,
    SmartPolicyChecks,
    SmartFrontend,
    CiFrontendCheckout,
    CiContractSource,
    CiWindowsSmoke,
    CiResourceGate,
    CiIntegration,
    CiRequiredJobs,
    CiSupplyChainSource,
    CiCargoAudit,
    CiCargoDeny,
    CiCycloneDxReport,
    CiTrivyReport,
    CiDeploymentChanges,
    CiDeploymentStatic,
    CiDeploymentCompose,
    CiDeploymentNginx,
    CiDeploymentPrometheus,
    CiDeploymentImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TaskDefinition {
    pub(crate) executor: TaskExecutor,
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) repository: TaskRepository,
    pub(crate) stage: TaskStage,
    pub(crate) working_directory: TaskWorkingDirectory,
    pub(crate) static_arguments: Option<&'static [&'static str]>,
    pub(crate) compilation_coverage: &'static [&'static str],
    pub(crate) allowed_writes: &'static [&'static str],
    pub(crate) external_resources: &'static [&'static str],
}

macro_rules! task_definition {
    ($executor:ident, $id:literal, $label:literal, $description:literal, $repository:ident,
     $stage:ident, $directory:ident, $arguments:expr, $coverage:expr, $writes:expr,
     $resources:expr) => {
        TaskDefinition {
            executor: TaskExecutor::$executor,
            id: $id,
            label: $label,
            description: $description,
            repository: TaskRepository::$repository,
            stage: TaskStage::$stage,
            working_directory: TaskWorkingDirectory::$directory,
            static_arguments: $arguments,
            compilation_coverage: $coverage,
            allowed_writes: $writes,
            external_resources: $resources,
        }
    };
}

#[path = "task_registry/ci.rs"]
mod ci;

const CHECK_TASKS: [TaskDefinition; 18] = [
    task_definition!(
        RemovedIdentity,
        "policy.removed-identity",
        "check_removed_identity",
        "核对前后端已移除的服务身份能力",
        CrossRepository,
        Static,
        Backend,
        None,
        &[],
        &[],
        &[]
    ),
    task_definition!(
        FrontendDependencies,
        "frontend.dependencies",
        "require_frontend_dependencies",
        "确认前端依赖已按锁文件安装",
        CrossRepository,
        Prerequisite,
        Frontend,
        None,
        &[],
        &[],
        &[]
    ),
    task_definition!(
        PythonEnvironment,
        "python.environment",
        "check_python_environment",
        "核验固定 Python 解释器与原生 AST 依赖",
        Backend,
        Prerequisite,
        Backend,
        None,
        &[],
        &[],
        &[]
    ),
    task_definition!(
        CargoFormat,
        "rust.format",
        "cargo_fmt",
        "检查 Rust Workspace 格式",
        Backend,
        Static,
        Backend,
        Some(&["fmt", "--all", "--", "--check"]),
        &[],
        &[],
        &[]
    ),
    task_definition!(
        FeatureRegistry,
        "rust.feature-registry",
        "check_feature_registry",
        "核对 Cargo feature 注册表与 Workspace",
        Backend,
        Static,
        Backend,
        None,
        &[],
        &[],
        &[]
    ),
    task_definition!(
        WorkspaceClippy,
        "rust.workspace-clippy",
        "workspace_clippy",
        "检查 Workspace 全目标和全部 feature",
        Backend,
        Static,
        Backend,
        None,
        &["Workspace Clippy --all-targets --all-features"],
        &["Cargo target"],
        &[]
    ),
    task_definition!(
        PolicyChecks,
        "policy.complete",
        "run_policy_checks",
        "执行完整后端政策检查集合",
        CrossRepository,
        Static,
        Backend,
        None,
        &[],
        &["Python 测试缓存"],
        &[]
    ),
    task_definition!(
        SnapshotPrepare,
        "contract.snapshots-prepare",
        "prepare_backend_snapshots",
        "准备当前后端候选快照",
        Backend,
        Contract,
        Backend,
        None,
        &[],
        &["候选 OpenAPI/MySQL 快照"],
        &[]
    ),
    task_definition!(
        PythonTests,
        "python.tests",
        "run_python_tests",
        "执行仓库 Python 检查测试",
        Backend,
        Test,
        Backend,
        None,
        &[],
        &["Python 测试缓存"],
        &[]
    ),
    task_definition!(
        MigrationHistory,
        "migration.history",
        "check_migration_history",
        "以 --require-frozen 核验冻结迁移历史",
        Backend,
        Contract,
        Backend,
        Some(STRICT_MIGRATION_HISTORY_ARGS),
        &[],
        &[],
        &[]
    ),
    task_definition!(
        WorkspaceGates,
        "rust.workspace-gates",
        "run_workspace_gates",
        "并行执行主 Workspace 与资源 Workspace 门禁",
        CrossRepository,
        Test,
        Backend,
        None,
        &[
            "feature matrix",
            "Workspace test --all-features",
            "资源生成器 default/schema-import"
        ],
        &["Cargo target", "并行任务日志", "资源 Workspace"],
        &[]
    ),
    task_definition!(
        SnapshotVerify,
        "contract.snapshots-verify",
        "verify_backend_snapshots",
        "核验候选快照与已提交事实一致",
        Backend,
        Contract,
        Backend,
        None,
        &[],
        &["候选后端快照"],
        &[]
    ),
    task_definition!(
        ConsumerContract,
        "contract.consumer",
        "run_consumer_contract",
        "使用候选 OpenAPI 执行前端契约检查",
        CrossRepository,
        Contract,
        Frontend,
        None,
        &[],
        &["前端契约检查产物"],
        &[]
    ),
    task_definition!(
        FrontendConsumerContract,
        "contract.consumer-full",
        "run_consumer_contract_full",
        "使用候选 OpenAPI 执行前端完整检查",
        CrossRepository,
        Build,
        Frontend,
        None,
        &[],
        &["前端测试报告", "前端生产构建"],
        &[]
    ),
    task_definition!(
        FrontendFull,
        "frontend.full",
        "run_frontend_full",
        "执行前端独立完整检查",
        Frontend,
        Build,
        Frontend,
        None,
        &[],
        &["前端测试报告", "前端生产构建"],
        &[]
    ),
    task_definition!(
        SmartBackendPackages,
        "smart.backend-packages",
        "run_smart_backend_packages",
        "检查选中包、反向依赖及 feature",
        Backend,
        Test,
        Backend,
        None,
        &["选中包及其 feature 测试"],
        &["Cargo target"],
        &[]
    ),
    task_definition!(
        SmartPolicyChecks,
        "policy.smart",
        "run_smart_policy_checks",
        "执行所选代码面的政策检查",
        CrossRepository,
        Static,
        Backend,
        None,
        &[],
        &["Python 测试缓存"],
        &[]
    ),
    task_definition!(
        SmartFrontend,
        "smart.frontend",
        "run_smart_frontend",
        "执行所选前端检查画像",
        Frontend,
        Test,
        Frontend,
        None,
        &[],
        &["前端测试报告", "前端生产构建"],
        &[]
    ),
];

const TASK_COUNT: usize = CHECK_TASKS.len() + ci::TASKS.len();

const fn task_registry() -> [TaskDefinition; TASK_COUNT] {
    let mut registry = [CHECK_TASKS[0]; TASK_COUNT];
    let mut index = 0;
    while index < CHECK_TASKS.len() {
        registry[index] = CHECK_TASKS[index];
        index += 1;
    }
    let mut ci_index = 0;
    while ci_index < ci::TASKS.len() {
        registry[index] = ci::TASKS[ci_index];
        index += 1;
        ci_index += 1;
    }
    registry
}

const TASK_REGISTRY_STORAGE: [TaskDefinition; TASK_COUNT] = task_registry();
pub(crate) const TASK_REGISTRY: &[TaskDefinition] = &TASK_REGISTRY_STORAGE;

impl TaskExecutor {
    pub(crate) fn definition(self) -> &'static TaskDefinition {
        TASK_REGISTRY
            .iter()
            .find(|definition| definition.executor == self)
            .expect("每个任务执行器必须在唯一注册表中登记")
    }

    pub(crate) fn label(self) -> &'static str {
        self.definition().label
    }

    pub(crate) fn description(self) -> &'static str {
        self.definition().description
    }

    pub(crate) fn static_arguments(self) -> Option<&'static [&'static str]> {
        self.definition().static_arguments
    }
}
