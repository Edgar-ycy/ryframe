use std::path::Path;

use crate::{
    Result,
    check::{
        BackendSnapshotProfile, TaskPlan, TaskRepository, TaskSpec, TaskStage,
        TaskWorkingDirectory, VerifySelection,
    },
    cli::CiCommand,
};

use super::{FULL_CI_EVENTS, INTEGRATION_PACKAGES};

const CI_TASKS: [CiTaskExecutor; 5] = [
    CiTaskExecutor::Preflight,
    CiTaskExecutor::RustGate,
    CiTaskExecutor::ResourceGate,
    CiTaskExecutor::Integration,
    CiTaskExecutor::ConsumerContract,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CiTaskExecutor {
    Preflight,
    RustGate,
    ResourceGate,
    Integration,
    ConsumerContract,
}

impl CiTaskExecutor {
    const fn output_name(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::RustGate => "rust_gate",
            Self::ResourceGate => "resource_gate",
            Self::Integration => "integration",
            Self::ConsumerContract => "consumer_contract",
        }
    }

    const fn command_name(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::RustGate => "rust-gate",
            Self::ResourceGate => "resource-gate",
            Self::Integration => "integration",
            Self::ConsumerContract => "consumer-contract",
        }
    }
}

pub(crate) fn ci_plan_for(
    event: &str,
    action: &str,
    selection: &VerifySelection,
    resource_gate: bool,
) -> Result<TaskPlan<CiTaskExecutor>> {
    if event == "pull_request" && action == "edited" {
        // PR 正文承载精确前端提交；编辑 marker 后必须重新核对跨仓删除策略。
        return task_plan(&[CiTaskExecutor::Preflight, CiTaskExecutor::ConsumerContract]);
    }
    if FULL_CI_EVENTS.contains(&event) {
        return task_plan(&CI_TASKS[..4]);
    }
    if selection.full_reason.is_some() {
        return task_plan(&CI_TASKS);
    }

    let has_backend_work =
        !selection.backend_packages.is_empty() || !selection.backend_snapshot_profiles.is_empty();
    let integration = selection
        .backend_packages
        .iter()
        .any(|package| INTEGRATION_PACKAGES.contains(&package.as_str()));
    let consumer_contract = selection
        .backend_snapshot_profiles
        .contains(&BackendSnapshotProfile::OpenApiContract);
    let selected = [
        (CiTaskExecutor::Preflight, true),
        (CiTaskExecutor::RustGate, has_backend_work),
        (CiTaskExecutor::ResourceGate, resource_gate),
        (CiTaskExecutor::Integration, integration),
        (CiTaskExecutor::ConsumerContract, consumer_contract),
    ]
    .into_iter()
    .filter_map(|(executor, enabled)| enabled.then_some(executor))
    .collect::<Vec<_>>();
    task_plan(&selected)
}

pub(crate) fn ci_execution_plan_for(command: &CiCommand) -> Result<TaskPlan<CiTaskExecutor>> {
    let executor = match command {
        CiCommand::Preflight => CiTaskExecutor::Preflight,
        CiCommand::RustGate => CiTaskExecutor::RustGate,
        CiCommand::ResourceGate => CiTaskExecutor::ResourceGate,
        CiCommand::Integration => CiTaskExecutor::Integration,
        CiCommand::ConsumerContract => CiTaskExecutor::ConsumerContract,
        CiCommand::Plan | CiCommand::ResourceGateReplay(_) => {
            return Err("该 CI 子命令不是独立 GitHub job 任务".into());
        }
    };
    task_plan(&[executor])
}

pub(crate) fn plan_outputs(plan: &TaskPlan<CiTaskExecutor>) -> [(&'static str, bool); 5] {
    CI_TASKS.map(|executor| (executor.output_name(), plan.contains(&executor)))
}

pub(super) fn execute_ci_job(plan: &TaskPlan<CiTaskExecutor>, frontend_dir: &Path) -> Result<()> {
    let [task] = plan.tasks.as_slice() else {
        return Err("每次 check ci 执行必须精确选择一个独立 job 任务".into());
    };
    println!(
        "开始 CI 任务：{}（cargo xtask check ci {}）",
        task.id,
        task.executor.command_name()
    );
    match task.executor {
        CiTaskExecutor::Preflight => super::preflight(frontend_dir),
        CiTaskExecutor::RustGate => super::rust_gate(frontend_dir),
        CiTaskExecutor::ResourceGate => super::resource_gate::run(frontend_dir),
        CiTaskExecutor::Integration => super::integration(),
        CiTaskExecutor::ConsumerContract => super::consumer_contract(frontend_dir),
    }
}

fn task_plan(selected: &[CiTaskExecutor]) -> Result<TaskPlan<CiTaskExecutor>> {
    let tasks = CI_TASKS
        .into_iter()
        .filter(|executor| selected.contains(executor))
        .map(task_spec)
        .collect();
    TaskPlan::new(tasks)
}

fn task_spec(executor: CiTaskExecutor) -> TaskSpec<CiTaskExecutor> {
    let (repository, stage, compilation_coverage, allowed_writes, external_resources) =
        match executor {
            CiTaskExecutor::Preflight => (
                TaskRepository::CrossRepository,
                TaskStage::Static,
                &[][..],
                &["Python 测试缓存"][..],
                &[][..],
            ),
            CiTaskExecutor::RustGate => (
                TaskRepository::CrossRepository,
                TaskStage::Test,
                &["Workspace Clippy/test/feature matrix", "资源 Workspace"][..],
                &["Cargo target", "候选后端快照"][..],
                &[][..],
            ),
            CiTaskExecutor::ResourceGate => (
                TaskRepository::CrossRepository,
                TaskStage::Test,
                &["资源反向依赖闭包"][..],
                &["Cargo target", "资源门禁报告"][..],
                &[][..],
            ),
            CiTaskExecutor::Integration => (
                TaskRepository::Backend,
                TaskStage::Test,
                &["MySQL、Redis 与 TLS 真实协议目标"][..],
                &["Cargo target", "隔离测试资源"][..],
                &["已登记的 MySQL、Redis 与 TLS 测试资源"][..],
            ),
            CiTaskExecutor::ConsumerContract => (
                TaskRepository::CrossRepository,
                TaskStage::Contract,
                &["OpenAPI 生产者与前端消费者"][..],
                &["候选 OpenAPI", "前端检查产物"][..],
                &[][..],
            ),
        };
    TaskSpec {
        id: executor.output_name(),
        dependencies: Vec::new(),
        repository,
        stage,
        working_directory: TaskWorkingDirectory::Backend,
        executor,
        compilation_coverage,
        allowed_writes,
        external_resources,
    }
}
