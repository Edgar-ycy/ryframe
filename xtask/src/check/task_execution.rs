use std::collections::BTreeSet;

use super::{
    backend_packages, frontend_profiles, require_frontend_dependencies, run_parallel_tasks,
    run_removed_identity,
};
use crate::{
    Result,
    migration::run_check as run_migration_history_check,
    process::{run as run_process, run_owned, run_owned_with_env, run_pnpm},
};

use super::super::{
    cargo_command::{
        ci_test_jobs_from, default_test_jobs_from, workspace_clippy_args, workspace_test_args,
    },
    context::VerifyExecutionContext,
    feature::{check_feature_registry, feature_matrix_with_jobs},
    model::BackendSnapshotProfile,
    node_suite::run_node_tests,
    plan::{CheckPlanMode, CheckTaskPlan},
    policy_tasks::{PolicyProfile, PolicyTaskInvocation, policy_tasks},
    resource::resource_workspace_compilation,
    snapshot::{
        BackendSnapshots, export_and_verify_backend_snapshots, run_consumer_contract,
        verify_backend_snapshots,
    },
    task_plan::TaskExecutor,
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum TaskExecutionMode<'a> {
    Check(&'a CheckPlanMode),
    Ci,
}

#[derive(Default)]
pub(crate) struct CheckExecutionState {
    snapshots: Option<BackendSnapshots>,
    consumer_contract_ran: bool,
}

pub(super) fn execute_plan(plan: &CheckTaskPlan, context: &VerifyExecutionContext) -> Result<()> {
    let mut state = CheckExecutionState::default();
    for task in &plan.task_plan.tasks {
        println!("开始检查任务：{}", task.id);
        execute_registered_task(
            task.executor,
            TaskExecutionMode::Check(&plan.mode),
            context,
            &mut state,
        )?;
    }
    if plan.task_plan.tasks.is_empty() {
        println!("没有需要执行的代码检查；当前变更仅包含文档，或工作树没有变更。");
    }
    Ok(())
}

pub(crate) fn execute_registered_task(
    executor: TaskExecutor,
    mode: TaskExecutionMode<'_>,
    context: &VerifyExecutionContext,
    state: &mut CheckExecutionState,
) -> Result<()> {
    match executor {
        TaskExecutor::BackendDocumentation => super::super::architecture::docs::run(&context.root),
        TaskExecutor::FrontendDocumentation => run_process(
            &context.frontend_dir,
            "node",
            &["scripts/check-documentation.mjs"],
        ),
        TaskExecutor::RemovedIdentity => run_removed_identity(
            &context.root,
            context
                .include_frontend
                .then_some(context.frontend_dir.as_path()),
        ),
        TaskExecutor::PrereleaseDependencies => super::super::prerelease::run(&context.root),
        TaskExecutor::FrontendDependencies => require_frontend_dependencies(&context.frontend_dir),
        TaskExecutor::PythonEnvironment => super::super::python_environment::run(&context.root),
        TaskExecutor::CargoFormat => run_process(
            &context.root,
            "cargo",
            executor.static_arguments().ok_or("Rust 格式任务缺少参数")?,
        ),
        TaskExecutor::FeatureRegistry => check_feature_registry(&context.root),
        TaskExecutor::WorkspaceClippy => run_workspace_clippy(context),
        TaskExecutor::DeploymentAssets => {
            super::super::deployment_assets::run_source(&context.root)
        }
        TaskExecutor::PermissionRoutes => super::super::permission_routes::run(&context.root),
        TaskExecutor::PolicyChecks | TaskExecutor::BackendPolicyChecks => {
            run_policy_checks(context, policy_profile(mode))
        }
        TaskExecutor::PythonTests => super::super::python_suite::run(&context.root),
        TaskExecutor::NodeTests => run_node_tests(&context.root),
        TaskExecutor::MigrationHistory => run_migration_history(context, mode),
        TaskExecutor::WorkspaceGates => run_workspace_gates(context, mode, state, true),
        TaskExecutor::BackendWorkspaceGates => run_workspace_gates(context, mode, state, false),
        TaskExecutor::FrontendFull => run_pnpm(&context.frontend_dir, &["check", "--full"]),
        TaskExecutor::SnapshotPrepare
        | TaskExecutor::SnapshotVerify
        | TaskExecutor::ConsumerContract
        | TaskExecutor::FrontendConsumerContract => {
            execute_snapshot_task(executor, mode, context, state)
        }
        TaskExecutor::SmartBackendPackages
        | TaskExecutor::SmartPolicyChecks
        | TaskExecutor::BackendSmartPolicyChecks
        | TaskExecutor::SmartFrontend => execute_smart_task(executor, mode, context, state),
        TaskExecutor::CiFrontendSource
        | TaskExecutor::CiFrontendCandidateSource
        | TaskExecutor::CiFrontendCheckout
        | TaskExecutor::CiContractSource
        | TaskExecutor::CiWindowsSmoke
        | TaskExecutor::CiResourceGate
        | TaskExecutor::CiIntegration
        | TaskExecutor::CiRequiredJobs
        | TaskExecutor::CiSupplyChainSource
        | TaskExecutor::CiCargoAudit
        | TaskExecutor::CiCargoDeny
        | TaskExecutor::CiCycloneDxReport
        | TaskExecutor::CiTrivyReport
        | TaskExecutor::CiDeploymentChanges
        | TaskExecutor::CiDeploymentStatic
        | TaskExecutor::CiDeploymentCompose
        | TaskExecutor::CiDeploymentNginx
        | TaskExecutor::CiDeploymentPrometheus
        | TaskExecutor::CiDeploymentImage => Err("CI 专属节点必须由 CI job 适配器执行".into()),
        TaskExecutor::ReleaseSource | TaskExecutor::ReleaseCi => {
            Err("发布专属节点必须由发布适配器执行".into())
        }
        TaskExecutor::LanguageReport
        | TaskExecutor::LanguageWrite
        | TaskExecutor::LanguageThreshold => Err("语言专属节点必须由语言报告适配器执行".into()),
        TaskExecutor::PerfCgroupRun | TaskExecutor::PerfCgroupCleanup => {
            Err("性能专属节点必须由 DevEx 适配器执行".into())
        }
    }
}

fn execute_snapshot_task(
    executor: TaskExecutor,
    mode: TaskExecutionMode<'_>,
    context: &VerifyExecutionContext,
    state: &mut CheckExecutionState,
) -> Result<()> {
    match executor {
        TaskExecutor::SnapshotPrepare => {
            state.snapshots = Some(export_and_verify_backend_snapshots(
                &context.root,
                &snapshot_profiles(mode),
                context.targets.backend.as_str(),
            )?);
            Ok(())
        }
        TaskExecutor::SnapshotVerify => prepare_or_verify_snapshots(context, mode, state),
        TaskExecutor::ConsumerContract => run_selected_consumer(context, mode, state, false),
        TaskExecutor::FrontendConsumerContract => run_selected_consumer(context, mode, state, true),
        _ => Err("非快照任务不能由快照执行器运行".into()),
    }
}

fn execute_smart_task(
    executor: TaskExecutor,
    mode: TaskExecutionMode<'_>,
    context: &VerifyExecutionContext,
    state: &mut CheckExecutionState,
) -> Result<()> {
    let TaskExecutionMode::Check(CheckPlanMode::Selected(selection)) = mode else {
        return Err("智能检查节点只能出现在智能任务图中".into());
    };
    match executor {
        TaskExecutor::SmartBackendPackages => backend_packages(
            context,
            &selection.backend_packages,
            state.snapshots.as_ref(),
        ),
        TaskExecutor::SmartPolicyChecks | TaskExecutor::BackendSmartPolicyChecks => {
            run_policy_checks(context, PolicyProfile::Smart)
        }
        TaskExecutor::SmartFrontend => frontend_profiles(
            &context.frontend_dir,
            &selection.frontend_profiles,
            state.consumer_contract_ran,
        ),
        _ => Err("非智能任务不能由智能执行器运行".into()),
    }
}

fn run_workspace_clippy(context: &VerifyExecutionContext) -> Result<()> {
    let arguments = workspace_clippy_args(&context.targets.backend, context.jobs.backend);
    run_owned(&context.root, "cargo", &arguments)
}

fn run_policy_checks(context: &VerifyExecutionContext, profile: PolicyProfile) -> Result<()> {
    run_removed_identity(
        &context.root,
        context
            .include_frontend
            .then_some(context.frontend_dir.as_path()),
    )?;
    if profile != PolicyProfile::Smart {
        super::super::prerelease::run(&context.root)?;
    }
    super::super::architecture::run(&context.root)?;
    for task in policy_tasks(profile) {
        match task.invocation {
            PolicyTaskInvocation::SupplyChain => {
                super::super::supply_chain::run(&context.root, false)?;
            }
            PolicyTaskInvocation::MigrationHistory => {
                run_migration_history_check(&context.root, false, "HEAD")?;
            }
        }
    }
    Ok(())
}

fn policy_profile(mode: TaskExecutionMode<'_>) -> PolicyProfile {
    match mode {
        TaskExecutionMode::Ci => PolicyProfile::CiPreflight,
        TaskExecutionMode::Check(_) => PolicyProfile::FullStatic,
    }
}

fn run_migration_history(
    context: &VerifyExecutionContext,
    mode: TaskExecutionMode<'_>,
) -> Result<()> {
    let trusted_ref = if matches!(mode, TaskExecutionMode::Ci) {
        let base = std::env::var("RYFRAME_CI_BASE_SHA")
            .or_else(|_| std::env::var("GITHUB_BASE_SHA"))
            .ok();
        preflight_trusted_ref(base.as_deref()).unwrap_or_else(|| "HEAD".to_owned())
    } else {
        "HEAD".to_owned()
    };
    run_migration_history_check(&context.root, true, &trusted_ref)
}

pub(crate) fn preflight_trusted_ref(base: Option<&str>) -> Option<String> {
    base.filter(|value| valid_git_sha(value)).map(str::to_owned)
}

fn valid_git_sha(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn snapshot_profiles(mode: TaskExecutionMode<'_>) -> BTreeSet<BackendSnapshotProfile> {
    match mode {
        TaskExecutionMode::Check(CheckPlanMode::Selected(selection)) => {
            selection.backend_snapshot_profiles.clone()
        }
        TaskExecutionMode::Check(CheckPlanMode::ExplicitFull | CheckPlanMode::ExpandedFull(_))
        | TaskExecutionMode::Ci => full_snapshot_profiles(),
    }
}

fn full_snapshot_profiles() -> BTreeSet<BackendSnapshotProfile> {
    [
        BackendSnapshotProfile::OpenApiContract,
        BackendSnapshotProfile::Mysql,
    ]
    .into_iter()
    .collect()
}

fn prepare_or_verify_snapshots(
    context: &VerifyExecutionContext,
    mode: TaskExecutionMode<'_>,
    state: &mut CheckExecutionState,
) -> Result<()> {
    if let Some(generated) = state.snapshots.as_ref() {
        return verify_backend_snapshots(&context.root, generated);
    }
    state.snapshots = Some(export_and_verify_backend_snapshots(
        &context.root,
        &snapshot_profiles(mode),
        context.targets.backend.as_str(),
    )?);
    Ok(())
}

fn run_workspace_gates(
    context: &VerifyExecutionContext,
    mode: TaskExecutionMode<'_>,
    state: &CheckExecutionState,
    include_resource_workspace: bool,
) -> Result<()> {
    let snapshots = state
        .snapshots
        .as_ref()
        .ok_or("Workspace 门禁缺少后端快照")?;
    let budget = context.jobs;
    let test_jobs = match mode {
        TaskExecutionMode::Ci => ci_test_jobs_from(
            std::env::var("RYFRAME_CI_TEST_JOBS").ok().as_deref(),
            cfg!(windows),
            budget.total,
        )?,
        TaskExecutionMode::Check(_) => default_test_jobs_from(cfg!(windows), budget.backend),
    };
    println!(
        "Workspace 门禁并发：总计={}，主 Workspace 编译={}，测试={}，资源 Workspace={}",
        budget.total, budget.backend, test_jobs, budget.resource
    );
    if include_resource_workspace {
        run_workspace_branches(context, snapshots, test_jobs)
    } else {
        run_backend_workspace(context, snapshots, test_jobs)
    }
}

fn run_backend_workspace(
    context: &VerifyExecutionContext,
    snapshots: &BackendSnapshots,
    test_jobs: usize,
) -> Result<()> {
    feature_matrix_with_jobs(
        &context.root,
        context.targets.backend.as_str(),
        context.jobs.backend,
    )?;
    run_owned_with_env(
        &context.root,
        "cargo",
        &workspace_test_args(context.targets.backend.as_str(), test_jobs),
        &snapshots.workspace_test_environment(),
    )
}

fn run_workspace_branches(
    context: &VerifyExecutionContext,
    snapshots: &BackendSnapshots,
    test_jobs: usize,
) -> Result<()> {
    run_parallel_tasks(
        &context.root,
        "backend-workspace",
        || run_backend_workspace(context, snapshots, test_jobs),
        "resource-workspace",
        || {
            resource_workspace_compilation(
                &context.root,
                &context.frontend_dir,
                context.targets.resource.as_str(),
                context.jobs.resource,
            )
        },
    )
}

fn run_selected_consumer(
    context: &VerifyExecutionContext,
    mode: TaskExecutionMode<'_>,
    state: &mut CheckExecutionState,
    full: bool,
) -> Result<()> {
    if matches!(mode, TaskExecutionMode::Ci) && full {
        return Err("CI 消费契约 job 不得隐式执行前端完整检查".into());
    }
    require_frontend_dependencies(&context.frontend_dir)?;
    if state.snapshots.is_none() {
        state.snapshots = Some(export_and_verify_backend_snapshots(
            &context.root,
            &[BackendSnapshotProfile::OpenApiContract]
                .into_iter()
                .collect(),
            context.targets.backend.as_str(),
        )?);
    }
    run_consumer_contract(
        &context.root,
        &context.frontend_dir,
        state
            .snapshots
            .as_ref()
            .ok_or("消费契约缺少 OpenAPI 快照")?,
        full,
    )?;
    state.consumer_contract_ran = true;
    Ok(())
}
