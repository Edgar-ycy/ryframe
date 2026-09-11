use crate::{Result, cli::CheckScope};

use super::{
    super::{
        model::VerifySelection,
        selection::needs_consumer_contract,
        snapshot::package_tests_generate_snapshots,
        task_plan::{TaskExecutor, TaskPlan},
    },
    CheckPlanMode,
};

pub(crate) fn tasks_for(scope: CheckScope, mode: &CheckPlanMode) -> Result<TaskPlan> {
    let executors = match mode {
        CheckPlanMode::ExplicitFull | CheckPlanMode::ExpandedFull(_) => full_tasks(scope),
        CheckPlanMode::Selected(selection) => smart_tasks(selection),
    };
    TaskPlan::sequence(&executors)
}

fn full_tasks(scope: CheckScope) -> Vec<TaskExecutor> {
    if scope == CheckScope::Frontend {
        return vec![TaskExecutor::RemovedIdentity, TaskExecutor::FrontendFull];
    }
    let mut tasks = vec![
        TaskExecutor::PythonEnvironment,
        TaskExecutor::FrontendDependencies,
        TaskExecutor::FeatureRegistry,
        TaskExecutor::CargoFormat,
        TaskExecutor::WorkspaceClippy,
        TaskExecutor::PolicyChecks,
        TaskExecutor::SnapshotPrepare,
        TaskExecutor::PythonTests,
        TaskExecutor::MigrationHistory,
        TaskExecutor::WorkspaceGates,
        TaskExecutor::SnapshotVerify,
    ];
    tasks.push(match scope {
        CheckScope::Backend => TaskExecutor::ConsumerContract,
        CheckScope::All => TaskExecutor::FrontendConsumerContract,
        CheckScope::Frontend => unreachable!("前端完整任务已提前返回"),
    });
    tasks
}

fn smart_tasks(selection: &VerifySelection) -> Vec<TaskExecutor> {
    let prepare_snapshots = !selection.backend_packages.is_empty()
        && !selection.backend_snapshot_profiles.is_empty()
        && package_tests_generate_snapshots(
            &selection.backend_snapshot_profiles,
            &selection.backend_packages,
        );
    let mut tasks = Vec::new();
    if selection.backend_packages.is_empty() && !selection.frontend_profiles.is_empty() {
        tasks.push(TaskExecutor::RemovedIdentity);
    }
    if prepare_snapshots {
        tasks.push(TaskExecutor::SnapshotPrepare);
    }
    if !selection.backend_packages.is_empty() {
        tasks.extend([
            TaskExecutor::FeatureRegistry,
            TaskExecutor::CargoFormat,
            TaskExecutor::SmartBackendPackages,
            TaskExecutor::SmartPolicyChecks,
        ]);
    }
    if !selection.backend_snapshot_profiles.is_empty() {
        tasks.push(TaskExecutor::SnapshotVerify);
    }
    if needs_consumer_contract(&selection.backend_snapshot_profiles) {
        tasks.push(TaskExecutor::ConsumerContract);
    }
    if !selection.frontend_profiles.is_empty() {
        tasks.push(TaskExecutor::SmartFrontend);
    }
    tasks
}
