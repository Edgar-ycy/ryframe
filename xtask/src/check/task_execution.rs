use std::collections::BTreeSet;

use super::{
    PYTHON_TEST_ARGS, backend, backend_packages, frontend_profiles, require_frontend_dependencies,
    run_parallel_tasks,
};
use crate::{
    Result,
    process::{run as run_process, run_owned_with_env, run_pnpm},
};

use super::super::{
    cargo_command::{default_test_jobs_from, workspace_test_args},
    context::VerifyExecutionContext,
    feature::feature_matrix_with_jobs,
    model::{BackendSnapshotProfile, VerifySelection},
    plan::{CheckPlanMode, CheckTaskExecutor, TaskPlan},
    resource::resource_workspace_compilation,
    snapshot::{
        BackendSnapshots, export_and_verify_backend_snapshots, prepare_backend_snapshots,
        run_consumer_contract, verify_backend_snapshots,
    },
};

#[derive(Default)]
struct CheckExecutionState {
    snapshots: Option<BackendSnapshots>,
    consumer_contract_ran: bool,
}

pub(super) fn execute_plan(plan: &TaskPlan, context: &VerifyExecutionContext) -> Result<()> {
    let mut state = CheckExecutionState::default();
    for task in &plan.tasks {
        println!("开始检查任务：{}", task.id);
        execute_task(task.executor, plan, context, &mut state)?;
    }
    if plan.tasks.is_empty() {
        println!("没有需要执行的代码检查；当前变更仅包含文档，或工作树没有变更。");
    }
    Ok(())
}

fn execute_task(
    executor: CheckTaskExecutor,
    plan: &TaskPlan,
    context: &VerifyExecutionContext,
    state: &mut CheckExecutionState,
) -> Result<()> {
    match executor {
        CheckTaskExecutor::SmartSnapshotPrepare
        | CheckTaskExecutor::SmartBackendPackages
        | CheckTaskExecutor::SmartSnapshotVerify
        | CheckTaskExecutor::SmartConsumerContract
        | CheckTaskExecutor::SmartFrontend => execute_smart_task(executor, plan, context, state),
        _ => execute_full_task(executor, context, state),
    }
}

fn execute_smart_task(
    executor: CheckTaskExecutor,
    plan: &TaskPlan,
    context: &VerifyExecutionContext,
    state: &mut CheckExecutionState,
) -> Result<()> {
    let CheckPlanMode::Selected(selection) = &plan.mode else {
        return Err("智能检查节点只能出现在智能任务图中".into());
    };
    match executor {
        CheckTaskExecutor::SmartSnapshotPrepare => {
            state.snapshots = Some(prepare_backend_snapshots(
                &context.root,
                &selection.backend_snapshot_profiles,
            )?);
            Ok(())
        }
        CheckTaskExecutor::SmartBackendPackages => backend_packages(
            context,
            &selection.backend_packages,
            state.snapshots.as_ref(),
        ),
        CheckTaskExecutor::SmartSnapshotVerify => {
            prepare_or_verify_smart_snapshots(context, selection, state)
        }
        CheckTaskExecutor::SmartConsumerContract => {
            require_frontend_dependencies(&context.frontend_dir)?;
            run_consumer_contract(
                &context.root,
                &context.frontend_dir,
                state.snapshots.as_ref().ok_or("后端快照尚未生成")?,
                false,
            )?;
            state.consumer_contract_ran = true;
            Ok(())
        }
        CheckTaskExecutor::SmartFrontend => frontend_profiles(
            &context.frontend_dir,
            &selection.frontend_profiles,
            state.consumer_contract_ran,
        ),
        _ => Err("完整检查节点不能由智能任务执行器运行".into()),
    }
}

fn prepare_or_verify_smart_snapshots(
    context: &VerifyExecutionContext,
    selection: &VerifySelection,
    state: &mut CheckExecutionState,
) -> Result<()> {
    if let Some(generated) = state.snapshots.as_ref() {
        return verify_backend_snapshots(&context.root, generated);
    }
    state.snapshots = Some(export_and_verify_backend_snapshots(
        &context.root,
        &selection.backend_snapshot_profiles,
        context.targets.backend.as_str(),
    )?);
    Ok(())
}

fn execute_full_task(
    executor: CheckTaskExecutor,
    context: &VerifyExecutionContext,
    state: &mut CheckExecutionState,
) -> Result<()> {
    match executor {
        CheckTaskExecutor::RequireFrontendDependencies => {
            require_frontend_dependencies(&context.frontend_dir)
        }
        CheckTaskExecutor::FullBackendStatic => backend(
            &context.root,
            context.targets.backend.as_str(),
            context.jobs.backend,
        ),
        CheckTaskExecutor::FullSnapshotPrepare => {
            state.snapshots = Some(prepare_backend_snapshots(
                &context.root,
                &full_snapshot_profiles(),
            )?);
            Ok(())
        }
        CheckTaskExecutor::FullPythonTests => {
            run_process(&context.root, "python", PYTHON_TEST_ARGS)
        }
        CheckTaskExecutor::FullMigrationHistory => run_process(
            &context.root,
            "python",
            &["scripts/check_migration_history.py", "--require-frozen"],
        ),
        CheckTaskExecutor::FullWorkspaces => run_full_workspaces(
            context,
            state.snapshots.as_ref().ok_or("完整门禁缺少后端快照")?,
        ),
        CheckTaskExecutor::FullSnapshotVerify => verify_backend_snapshots(
            &context.root,
            state.snapshots.as_ref().ok_or("完整门禁缺少后端快照")?,
        ),
        CheckTaskExecutor::FullBackendConsumerContract => run_full_consumer(context, state, false),
        CheckTaskExecutor::FullFrontendConsumerContract => run_full_consumer(context, state, true),
        CheckTaskExecutor::FullFrontend => run_pnpm(&context.frontend_dir, &["check", "--full"]),
        _ => Err("智能检查节点不能由完整任务执行器运行".into()),
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

fn run_full_workspaces(
    context: &VerifyExecutionContext,
    snapshots: &BackendSnapshots,
) -> Result<()> {
    let budget = context.jobs;
    let test_jobs = default_test_jobs_from(cfg!(windows), budget.backend);
    println!(
        "完整门禁并发：总计={}，主 Workspace 编译={}，测试={}，资源 Workspace={}",
        budget.total, budget.backend, test_jobs, budget.resource
    );
    run_parallel_tasks(
        &context.root,
        "backend-workspace",
        || {
            feature_matrix_with_jobs(
                &context.root,
                context.targets.backend.as_str(),
                budget.backend,
            )?;
            let args = workspace_test_args(context.targets.backend.as_str(), test_jobs);
            run_owned_with_env(
                &context.root,
                "cargo",
                &args,
                &snapshots.workspace_test_environment(),
            )
        },
        "resource-workspace",
        || {
            resource_workspace_compilation(
                &context.root,
                &context.frontend_dir,
                context.targets.resource.as_str(),
                budget.resource,
            )
        },
    )
}

fn run_full_consumer(
    context: &VerifyExecutionContext,
    state: &CheckExecutionState,
    full: bool,
) -> Result<()> {
    run_consumer_contract(
        &context.root,
        &context.frontend_dir,
        state
            .snapshots
            .as_ref()
            .ok_or("完整后端门禁缺少 OpenAPI 快照")?,
        full,
    )
}
