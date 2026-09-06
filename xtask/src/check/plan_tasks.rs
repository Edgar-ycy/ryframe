use std::collections::BTreeSet;

use crate::{Result, cli::CheckScope};

use super::{
    super::{
        model::VerifySelection, selection::needs_consumer_contract,
        snapshot::package_tests_generate_snapshots,
    },
    CheckPlanMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckTaskRepository {
    Backend,
    Frontend,
    CrossRepository,
}

impl CheckTaskRepository {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Backend => "后端",
            Self::Frontend => "前端",
            Self::CrossRepository => "跨仓",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckTaskStage {
    Prerequisite,
    Static,
    Test,
    Contract,
    Build,
}

impl CheckTaskStage {
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
pub(crate) enum CheckTaskWorkingDirectory {
    Backend,
    Frontend,
}

impl CheckTaskWorkingDirectory {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Backend => "后端工作区",
            Self::Frontend => "前端工作区",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckTaskExecutor {
    RemovedIdentity,
    RequireFrontendDependencies,
    FullPythonEnvironment,
    FullBackendStatic,
    FullSnapshotPrepare,
    FullPythonTests,
    FullMigrationHistory,
    FullWorkspaces,
    FullSnapshotVerify,
    FullBackendConsumerContract,
    FullFrontendConsumerContract,
    FullFrontend,
    SmartSnapshotPrepare,
    SmartBackendPackages,
    SmartSnapshotVerify,
    SmartConsumerContract,
    SmartFrontend,
}

impl CheckTaskExecutor {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::RemovedIdentity => "check_removed_identity",
            Self::RequireFrontendDependencies => "require_frontend_dependencies",
            Self::FullPythonEnvironment => "check_python_environment",
            Self::FullBackendStatic => "run_full_backend_static",
            Self::FullSnapshotPrepare => "prepare_full_snapshots",
            Self::FullPythonTests => "run_full_python_tests",
            Self::FullMigrationHistory => "run_full_migration_history",
            Self::FullWorkspaces => "run_full_workspaces_parallel",
            Self::FullSnapshotVerify => "verify_full_snapshots",
            Self::FullBackendConsumerContract => "run_backend_consumer_contract",
            Self::FullFrontendConsumerContract => "run_frontend_consumer_contract_full",
            Self::FullFrontend => "run_frontend_full",
            Self::SmartSnapshotPrepare => "prepare_smart_snapshots",
            Self::SmartBackendPackages => "run_smart_backend_packages",
            Self::SmartSnapshotVerify => "verify_smart_snapshots",
            Self::SmartConsumerContract => "run_smart_consumer_contract",
            Self::SmartFrontend => "run_smart_frontend",
        }
    }

    pub(crate) const fn description(self) -> &'static str {
        match self {
            Self::RemovedIdentity => "核对前后端已移除的服务身份能力",
            Self::RequireFrontendDependencies => "确认前端依赖已按锁文件安装",
            Self::FullPythonEnvironment => "核验固定 Python 解释器与原生 AST 依赖",
            Self::FullBackendStatic => "执行后端 feature 注册、格式、Clippy 与策略检查",
            Self::FullSnapshotPrepare => "准备当前后端 OpenAPI 与 MySQL 候选快照",
            Self::FullPythonTests => "执行仓库 Python 检查测试",
            Self::FullMigrationHistory => "核验冻结迁移历史",
            Self::FullWorkspaces => "并行执行后端 Workspace 与资源 Workspace 门禁",
            Self::FullSnapshotVerify => "核验候选快照与已提交事实一致",
            Self::FullBackendConsumerContract => "使用候选 OpenAPI 执行前端契约检查",
            Self::FullFrontendConsumerContract => "使用候选 OpenAPI 执行前端完整检查",
            Self::FullFrontend => "执行前端独立完整检查",
            Self::SmartSnapshotPrepare => "为选中后端包准备候选快照",
            Self::SmartBackendPackages => "检查选中包、反向依赖及 feature",
            Self::SmartSnapshotVerify => "生成或复用候选快照并核验",
            Self::SmartConsumerContract => "使用候选 OpenAPI 执行前端契约检查",
            Self::SmartFrontend => "执行所选前端检查画像",
        }
    }

    const fn working_directory(self) -> CheckTaskWorkingDirectory {
        match self {
            Self::RemovedIdentity => CheckTaskWorkingDirectory::Backend,
            Self::RequireFrontendDependencies
            | Self::FullBackendConsumerContract
            | Self::FullFrontendConsumerContract
            | Self::FullFrontend
            | Self::SmartConsumerContract
            | Self::SmartFrontend => CheckTaskWorkingDirectory::Frontend,
            _ => CheckTaskWorkingDirectory::Backend,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckTask {
    pub(crate) id: &'static str,
    pub(crate) dependencies: Vec<&'static str>,
    pub(crate) repository: CheckTaskRepository,
    pub(crate) stage: CheckTaskStage,
    pub(crate) working_directory: CheckTaskWorkingDirectory,
    pub(crate) executor: CheckTaskExecutor,
    pub(crate) compilation_coverage: &'static [&'static str],
    pub(crate) allowed_writes: &'static [&'static str],
    pub(crate) external_resources: &'static [&'static str],
}

pub(crate) fn tasks_for(scope: CheckScope, mode: &CheckPlanMode) -> Vec<CheckTask> {
    match mode {
        CheckPlanMode::ExplicitFull | CheckPlanMode::ExpandedFull(_) => full_tasks(scope),
        CheckPlanMode::Selected(selection) => smart_tasks(selection),
    }
}

fn full_tasks(scope: CheckScope) -> Vec<CheckTask> {
    let mut tasks = if matches!(scope, CheckScope::All | CheckScope::Backend) {
        full_backend_tasks()
    } else {
        Vec::new()
    };
    match scope {
        CheckScope::Backend => tasks.push(task(
            "full.consumer-contract",
            &["full.snapshots-verify"],
            CheckTaskRepository::CrossRepository,
            CheckTaskStage::Contract,
            CheckTaskExecutor::FullBackendConsumerContract,
            &[],
            &["前端契约检查产物"],
        )),
        CheckScope::All => tasks.push(task(
            "full.frontend",
            &["full.snapshots-verify"],
            CheckTaskRepository::CrossRepository,
            CheckTaskStage::Build,
            CheckTaskExecutor::FullFrontendConsumerContract,
            &[],
            &["前端测试报告", "前端生产构建"],
        )),
        CheckScope::Frontend => {
            tasks.push(removed_identity_task("full.removed-identity", &[]));
            tasks.push(task(
                "full.frontend",
                &["full.removed-identity"],
                CheckTaskRepository::Frontend,
                CheckTaskStage::Build,
                CheckTaskExecutor::FullFrontend,
                &[],
                &["前端测试报告", "前端生产构建"],
            ));
        }
    }
    tasks
}

fn full_backend_tasks() -> Vec<CheckTask> {
    vec![
        task(
            "full.python-environment",
            &[],
            CheckTaskRepository::Backend,
            CheckTaskStage::Prerequisite,
            CheckTaskExecutor::FullPythonEnvironment,
            &[],
            &[],
        ),
        task(
            "full.frontend-dependencies",
            &[],
            CheckTaskRepository::CrossRepository,
            CheckTaskStage::Prerequisite,
            CheckTaskExecutor::RequireFrontendDependencies,
            &[],
            &[],
        ),
        task(
            "full.backend-static",
            &["full.python-environment", "full.frontend-dependencies"],
            CheckTaskRepository::Backend,
            CheckTaskStage::Static,
            CheckTaskExecutor::FullBackendStatic,
            &["Workspace Clippy --all-targets --all-features"],
            &["Cargo target", "进程日志"],
        ),
        task(
            "full.snapshots-prepare",
            &["full.backend-static"],
            CheckTaskRepository::Backend,
            CheckTaskStage::Contract,
            CheckTaskExecutor::FullSnapshotPrepare,
            &[],
            &["候选 OpenAPI/MySQL 快照"],
        ),
        task(
            "full.python-tests",
            &["full.snapshots-prepare"],
            CheckTaskRepository::Backend,
            CheckTaskStage::Test,
            CheckTaskExecutor::FullPythonTests,
            &[],
            &["Python 测试缓存"],
        ),
        task(
            "full.migration-history",
            &["full.python-tests"],
            CheckTaskRepository::Backend,
            CheckTaskStage::Contract,
            CheckTaskExecutor::FullMigrationHistory,
            &[],
            &[],
        ),
        task(
            "full.workspaces",
            &["full.migration-history"],
            CheckTaskRepository::CrossRepository,
            CheckTaskStage::Test,
            CheckTaskExecutor::FullWorkspaces,
            &[
                "feature matrix",
                "Workspace test --all-features",
                "资源生成器 default/schema-import",
            ],
            &["Cargo target", "并行任务日志", "资源 Workspace"],
        ),
        task(
            "full.snapshots-verify",
            &["full.workspaces"],
            CheckTaskRepository::Backend,
            CheckTaskStage::Contract,
            CheckTaskExecutor::FullSnapshotVerify,
            &[],
            &[],
        ),
    ]
}

fn smart_tasks(selection: &VerifySelection) -> Vec<CheckTask> {
    let prepare_snapshots = !selection.backend_packages.is_empty()
        && !selection.backend_snapshot_profiles.is_empty()
        && package_tests_generate_snapshots(
            &selection.backend_snapshot_profiles,
            &selection.backend_packages,
        );
    let mut tasks = Vec::new();
    let mut previous = None;
    if selection.backend_packages.is_empty() && !selection.frontend_profiles.is_empty() {
        tasks.push(removed_identity_task("smart.removed-identity", &[]));
        previous = Some("smart.removed-identity");
    }
    if prepare_snapshots {
        tasks.push(task(
            "smart.snapshots-prepare",
            &[],
            CheckTaskRepository::Backend,
            CheckTaskStage::Contract,
            CheckTaskExecutor::SmartSnapshotPrepare,
            &[],
            &["候选后端快照"],
        ));
        previous = Some("smart.snapshots-prepare");
    }
    if !selection.backend_packages.is_empty() {
        tasks.push(task(
            "smart.backend-packages",
            dependency(previous),
            CheckTaskRepository::Backend,
            CheckTaskStage::Test,
            CheckTaskExecutor::SmartBackendPackages,
            &["选中包及其 feature 测试"],
            &["Cargo target"],
        ));
        previous = Some("smart.backend-packages");
    }
    if !selection.backend_snapshot_profiles.is_empty() {
        tasks.push(task(
            "smart.snapshots-verify",
            dependency(previous),
            CheckTaskRepository::Backend,
            CheckTaskStage::Contract,
            CheckTaskExecutor::SmartSnapshotVerify,
            &[],
            &["候选后端快照"],
        ));
        previous = Some("smart.snapshots-verify");
    }
    if needs_consumer_contract(&selection.backend_snapshot_profiles) {
        tasks.push(task(
            "smart.consumer-contract",
            dependency(previous),
            CheckTaskRepository::CrossRepository,
            CheckTaskStage::Contract,
            CheckTaskExecutor::SmartConsumerContract,
            &[],
            &["前端契约检查产物"],
        ));
        previous = Some("smart.consumer-contract");
    }
    if !selection.frontend_profiles.is_empty() {
        tasks.push(task(
            "smart.frontend",
            dependency(previous),
            CheckTaskRepository::Frontend,
            CheckTaskStage::Test,
            CheckTaskExecutor::SmartFrontend,
            &[],
            &["前端测试报告", "前端生产构建"],
        ));
    }
    tasks
}

fn dependency(previous: Option<&'static str>) -> &'static [&'static str] {
    match previous {
        Some("smart.snapshots-prepare") => &["smart.snapshots-prepare"],
        Some("smart.removed-identity") => &["smart.removed-identity"],
        Some("smart.backend-packages") => &["smart.backend-packages"],
        Some("smart.snapshots-verify") => &["smart.snapshots-verify"],
        Some("smart.consumer-contract") => &["smart.consumer-contract"],
        Some(_) | None => &[],
    }
}

fn removed_identity_task(id: &'static str, dependencies: &'static [&'static str]) -> CheckTask {
    task(
        id,
        dependencies,
        CheckTaskRepository::CrossRepository,
        CheckTaskStage::Static,
        CheckTaskExecutor::RemovedIdentity,
        &[],
        &[],
    )
}

fn task(
    id: &'static str,
    dependencies: &'static [&'static str],
    repository: CheckTaskRepository,
    stage: CheckTaskStage,
    executor: CheckTaskExecutor,
    compilation_coverage: &'static [&'static str],
    allowed_writes: &'static [&'static str],
) -> CheckTask {
    CheckTask {
        id,
        dependencies: dependencies.to_vec(),
        repository,
        stage,
        working_directory: executor.working_directory(),
        executor,
        compilation_coverage,
        allowed_writes,
        external_resources: &[],
    }
}

pub(super) fn validate_task_graph(tasks: &[CheckTask]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for task in tasks {
        if !seen.insert(task.id) {
            return Err(format!("检查任务 ID 重复：{}", task.id).into());
        }
        for dependency in &task.dependencies {
            if !seen.contains(dependency) {
                return Err(format!(
                    "检查任务 {} 的依赖 {} 不存在或未按拓扑顺序声明",
                    task.id, dependency
                )
                .into());
            }
        }
    }
    Ok(())
}
