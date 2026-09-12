use std::collections::BTreeSet;

use super::{
    PYTHON_ENVIRONMENT_ARGS, PYTHON_TEST_ARGS, backend_packages, frontend_profiles,
    require_frontend_dependencies, run_parallel_tasks, run_removed_identity,
};
use crate::{
    Result,
    process::{run as run_process, run_owned, run_owned_with_env, run_pnpm},
};

use super::super::{
    cargo_command::{
        ci_test_jobs_from, default_test_jobs_from, workspace_clippy_args, workspace_test_args,
    },
    context::VerifyExecutionContext,
    feature::{check_feature_registry, feature_matrix_with_jobs},
    model::BackendSnapshotProfile,
    plan::{CheckPlanMode, CheckTaskPlan},
    policy_tasks::{PolicyProfile, policy_tasks},
    resource::resource_workspace_compilation,
    snapshot::{
        BackendSnapshots, export_and_verify_backend_snapshots, prepare_backend_snapshots,
        run_consumer_contract, verify_backend_snapshots,
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
        TaskExecutor::RemovedIdentity => run_removed_identity(&context.root, &context.frontend_dir),
        TaskExecutor::FrontendDependencies => require_frontend_dependencies(&context.frontend_dir),
        TaskExecutor::PythonEnvironment => {
            run_process(&context.root, "python", PYTHON_ENVIRONMENT_ARGS)
        }
        TaskExecutor::CargoFormat => run_process(
            &context.root,
            "cargo",
            executor.static_arguments().ok_or("Rust 格式任务缺少参数")?,
        ),
        TaskExecutor::FeatureRegistry => check_feature_registry(&context.root),
        TaskExecutor::WorkspaceClippy => run_workspace_clippy(context),
        TaskExecutor::PolicyChecks => run_policy_checks(context, policy_profile(mode)),
        TaskExecutor::PythonTests => run_process(&context.root, "python", PYTHON_TEST_ARGS),
        TaskExecutor::MigrationHistory => run_migration_history(context, mode, executor),
        TaskExecutor::WorkspaceGates => run_workspace_gates(context, mode, state),
        TaskExecutor::FrontendFull => run_pnpm(&context.frontend_dir, &["check", "--full"]),
        TaskExecutor::SnapshotPrepare
        | TaskExecutor::SnapshotVerify
        | TaskExecutor::ConsumerContract
        | TaskExecutor::FrontendConsumerContract => {
            execute_snapshot_task(executor, mode, context, state)
        }
        TaskExecutor::SmartBackendPackages
        | TaskExecutor::SmartPolicyChecks
        | TaskExecutor::SmartFrontend => execute_smart_task(executor, mode, context, state),
        TaskExecutor::CiFrontendCheckout
        | TaskExecutor::CiContractSource
        | TaskExecutor::CiWindowsSmoke
        | TaskExecutor::CiResourceGate
        | TaskExecutor::CiIntegration
        | TaskExecutor::CiRequiredJobs
        | TaskExecutor::CiSupplyChainSource
        | TaskExecutor::CiCargoAudit
        | TaskExecutor::CiCargoDeny => Err("CI 专属节点必须由 CI job 适配器执行".into()),
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
            state.snapshots = Some(prepare_backend_snapshots(
                &context.root,
                &snapshot_profiles(mode),
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
        TaskExecutor::SmartPolicyChecks => run_policy_checks(context, PolicyProfile::Smart),
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
    for task in policy_tasks(profile) {
        run_owned(
            &context.root,
            "python",
            &task.arguments(&context.root, &context.frontend_dir)?,
        )?;
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
    executor: TaskExecutor,
) -> Result<()> {
    if matches!(mode, TaskExecutionMode::Ci) {
        let base = std::env::var("RYFRAME_CI_BASE_SHA")
            .or_else(|_| std::env::var("GITHUB_BASE_SHA"))
            .ok();
        return run_owned(
            &context.root,
            "python",
            &preflight_migration_args(base.as_deref()),
        );
    }
    run_process(
        &context.root,
        "python",
        executor.static_arguments().ok_or("迁移历史任务缺少参数")?,
    )
}

pub(crate) fn preflight_migration_args(base: Option<&str>) -> Vec<String> {
    let mut arguments = super::super::policy_tasks::STRICT_MIGRATION_HISTORY_ARGS
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect::<Vec<_>>();
    if let Some(base) = base.filter(|value| valid_git_sha(value)) {
        arguments.extend(["--trusted-ref".to_owned(), base.to_owned()]);
    }
    arguments
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
    run_workspace_branches(context, snapshots, test_jobs)
}

fn run_workspace_branches(
    context: &VerifyExecutionContext,
    snapshots: &BackendSnapshots,
    test_jobs: usize,
) -> Result<()> {
    run_parallel_tasks(
        &context.root,
        "backend-workspace",
        || {
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
        },
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
