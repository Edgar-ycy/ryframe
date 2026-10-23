use crate::{Result, cli::CheckScope};

use super::{
    super::{
        model::{FrontendProfile, VerifySelection},
        selection::needs_consumer_contract,
        snapshot::package_tests_generate_snapshots,
        task_plan::{TaskExecutor, TaskPlan},
    },
    CheckPlanMode,
};

pub(crate) fn tasks_for(scope: CheckScope, mode: &CheckPlanMode) -> Result<TaskPlan> {
    let executors = match mode {
        CheckPlanMode::ExplicitFull | CheckPlanMode::ExpandedFull(_) => full_tasks(scope),
        CheckPlanMode::Selected(selection) => smart_tasks(selection, scope),
    };
    TaskPlan::sequence(&executors)
}

fn full_tasks(scope: CheckScope) -> Vec<TaskExecutor> {
    if scope == CheckScope::Frontend {
        return vec![TaskExecutor::RemovedIdentity, TaskExecutor::FrontendFull];
    }
    let mut tasks = vec![
        TaskExecutor::PythonEnvironment,
        TaskExecutor::FeatureRegistry,
        TaskExecutor::CargoFormat,
        TaskExecutor::WorkspaceClippy,
        TaskExecutor::DeploymentAssets,
        TaskExecutor::PermissionRoutes,
        if scope == CheckScope::Backend {
            TaskExecutor::BackendPolicyChecks
        } else {
            TaskExecutor::PolicyChecks
        },
        TaskExecutor::SnapshotPrepare,
        TaskExecutor::PythonTests,
        TaskExecutor::BackendSources,
        TaskExecutor::MigrationHistory,
        if scope == CheckScope::Backend {
            TaskExecutor::BackendWorkspaceGates
        } else {
            TaskExecutor::WorkspaceGates
        },
        TaskExecutor::SnapshotVerify,
    ];
    if scope == CheckScope::All {
        tasks.insert(1, TaskExecutor::FrontendDependencies);
        tasks.push(TaskExecutor::FrontendConsumerContract);
    }
    tasks
}

fn smart_tasks(selection: &VerifySelection, scope: CheckScope) -> Vec<TaskExecutor> {
    let prepare_snapshots = !selection.backend_packages.is_empty()
        && !selection.backend_snapshot_profiles.is_empty()
        && package_tests_generate_snapshots(
            &selection.backend_snapshot_profiles,
            &selection.backend_packages,
        );
    let mut tasks = Vec::new();
    if scope != CheckScope::Frontend
        && selection.backend_documentation
        && selection.backend_packages.is_empty()
    {
        tasks.push(TaskExecutor::BackendDocumentation);
    }
    if scope != CheckScope::Backend
        && selection.frontend_documentation
        && !selection.frontend_profiles.contains(&FrontendProfile::Code)
    {
        tasks.push(TaskExecutor::FrontendDocumentation);
    }
    if scope != CheckScope::Backend
        && selection.backend_packages.is_empty()
        && !selection.frontend_profiles.is_empty()
    {
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
            TaskExecutor::PermissionRoutes,
            if scope == CheckScope::Backend {
                TaskExecutor::BackendSmartPolicyChecks
            } else {
                TaskExecutor::SmartPolicyChecks
            },
        ]);
    }
    if !selection.backend_snapshot_profiles.is_empty() {
        tasks.push(TaskExecutor::SnapshotVerify);
    }
    if scope != CheckScope::Backend && needs_consumer_contract(&selection.backend_snapshot_profiles)
    {
        tasks.push(TaskExecutor::ConsumerContract);
    }
    if scope != CheckScope::Backend && !selection.frontend_profiles.is_empty() {
        tasks.push(TaskExecutor::SmartFrontend);
    }
    tasks
}
