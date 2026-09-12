use std::{env, path::Path};

use crate::{
    Result,
    check::{
        BackendSnapshotProfile, CheckExecutionState, TaskExecutionMode, TaskExecutor, TaskPlan,
        VerifyExecutionContext, VerifySelection, execute_registered_task,
    },
    cli::{CiCommand, SecurityCommand},
};

use super::{FULL_CI_EVENTS, INTEGRATION_PACKAGES, WINDOWS_RUST_GATE_PROFILE};

const PREFLIGHT_TASKS: &[TaskExecutor] = &[
    TaskExecutor::CargoFormat,
    TaskExecutor::PythonEnvironment,
    TaskExecutor::PythonTests,
    TaskExecutor::PolicyChecks,
    TaskExecutor::MigrationHistory,
];
const RUST_GATE_TASKS: &[TaskExecutor] = &[
    TaskExecutor::CiFrontendCheckout,
    TaskExecutor::SnapshotPrepare,
    TaskExecutor::FeatureRegistry,
    TaskExecutor::WorkspaceClippy,
    TaskExecutor::WorkspaceGates,
    TaskExecutor::SnapshotVerify,
];
const WINDOWS_RUST_GATE_TASKS: &[TaskExecutor] = &[
    TaskExecutor::CiFrontendCheckout,
    TaskExecutor::CiWindowsSmoke,
];
const RESOURCE_GATE_TASKS: &[TaskExecutor] = &[TaskExecutor::CiResourceGate];
const INTEGRATION_TASKS: &[TaskExecutor] = &[TaskExecutor::CiIntegration];
const CONSUMER_CONTRACT_TASKS: &[TaskExecutor] = &[
    TaskExecutor::CiFrontendCheckout,
    TaskExecutor::CiContractSource,
    TaskExecutor::FrontendDependencies,
    TaskExecutor::ConsumerContract,
];
const REQUIRED_TASKS: &[TaskExecutor] = &[TaskExecutor::CiRequiredJobs];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CiJob {
    Preflight,
    RustGate,
    ResourceGate,
    Integration,
    ConsumerContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CiJobDefinition {
    job: CiJob,
    output_name: &'static str,
    command_name: &'static str,
    workflow_job_name: Option<&'static str>,
    tasks: &'static [TaskExecutor],
}

const CI_JOBS: &[CiJobDefinition] = &[
    CiJobDefinition {
        job: CiJob::Preflight,
        output_name: "preflight",
        command_name: "preflight",
        workflow_job_name: None,
        tasks: PREFLIGHT_TASKS,
    },
    CiJobDefinition {
        job: CiJob::RustGate,
        output_name: "rust_gate",
        command_name: "rust-gate",
        workflow_job_name: Some("rust-gate"),
        tasks: RUST_GATE_TASKS,
    },
    CiJobDefinition {
        job: CiJob::ResourceGate,
        output_name: "resource_gate",
        command_name: "resource-gate",
        workflow_job_name: Some("resource-gate"),
        tasks: RESOURCE_GATE_TASKS,
    },
    CiJobDefinition {
        job: CiJob::Integration,
        output_name: "integration",
        command_name: "integration",
        workflow_job_name: Some("integration"),
        tasks: INTEGRATION_TASKS,
    },
    CiJobDefinition {
        job: CiJob::ConsumerContract,
        output_name: "consumer_contract",
        command_name: "consumer-contract",
        workflow_job_name: None,
        tasks: CONSUMER_CONTRACT_TASKS,
    },
];

impl CiJob {
    fn definition(self) -> &'static CiJobDefinition {
        CI_JOBS
            .iter()
            .find(|definition| definition.job == self)
            .expect("每个 CI job 必须在分组表中登记")
    }

    pub(crate) fn output_name(self) -> &'static str {
        self.definition().output_name
    }
}

pub(crate) fn required_ci_jobs() -> impl Iterator<Item = (CiJob, &'static str)> {
    CI_JOBS.iter().filter_map(|definition| {
        definition
            .workflow_job_name
            .map(|name| (definition.job, name))
    })
}

pub(crate) fn ci_plan_for(
    event: &str,
    action: &str,
    selection: &VerifySelection,
    resource_gate: bool,
) -> Result<Vec<CiJob>> {
    if event == "pull_request" && action == "edited" {
        // PR 正文承载精确前端提交；编辑 marker 后必须重新核对跨仓删除策略。
        return selected_jobs(&[CiJob::Preflight, CiJob::ConsumerContract]);
    }
    if FULL_CI_EVENTS.contains(&event) {
        return selected_jobs(&[
            CiJob::Preflight,
            CiJob::RustGate,
            CiJob::ResourceGate,
            CiJob::Integration,
        ]);
    }
    if selection.full_reason.is_some() {
        let jobs = CI_JOBS
            .iter()
            .map(|definition| definition.job)
            .collect::<Vec<_>>();
        return selected_jobs(&jobs);
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
    let jobs = [
        (CiJob::Preflight, true),
        (CiJob::RustGate, has_backend_work),
        (CiJob::ResourceGate, resource_gate),
        (CiJob::Integration, integration),
        (CiJob::ConsumerContract, consumer_contract),
    ]
    .into_iter()
    .filter_map(|(job, enabled)| enabled.then_some(job))
    .collect::<Vec<_>>();
    selected_jobs(&jobs)
}

pub(crate) fn ci_execution_plan_for(command: &CiCommand) -> Result<TaskPlan> {
    let profile = env::var("RYFRAME_CI_RUST_GATE_PROFILE").ok();
    ci_execution_plan_for_profile(command, profile.as_deref())
}

pub(crate) fn ci_execution_plan_for_profile(
    command: &CiCommand,
    rust_gate_profile: Option<&str>,
) -> Result<TaskPlan> {
    if matches!(command, CiCommand::Required(_)) {
        return TaskPlan::sequence(REQUIRED_TASKS);
    }
    if let CiCommand::Security(command) = command {
        return super::security::plan(command);
    }
    let job = job_for_command(command)?;
    let tasks = if job == CiJob::RustGate {
        match rust_gate_profile {
            None | Some("") | Some("standard") => job.definition().tasks,
            Some(WINDOWS_RUST_GATE_PROFILE) => WINDOWS_RUST_GATE_TASKS,
            Some(profile) => {
                return Err(format!(
                    "RYFRAME_CI_RUST_GATE_PROFILE 只允许 standard 或 \
                     {WINDOWS_RUST_GATE_PROFILE}，实际为 {profile}"
                )
                .into());
            }
        }
    } else {
        job.definition().tasks
    };
    TaskPlan::sequence(tasks)
}

pub(crate) fn plan_outputs(plan: &[CiJob]) -> [(&'static str, bool); 5] {
    CI_JOBS
        .iter()
        .map(|definition| (definition.output_name, plan.contains(&definition.job)))
        .collect::<Vec<_>>()
        .try_into()
        .expect("CI 输出契约固定为五项")
}

pub(super) fn execute_ci_job(
    command: &CiCommand,
    plan: &TaskPlan,
    frontend_dir: &Path,
) -> Result<()> {
    if let CiCommand::Required(options) = command {
        println!("开始 CI job：required（cargo xtask check ci required）");
        return execute_required_plan(options, plan);
    }
    if let CiCommand::Security(command) = command {
        let name = match command {
            SecurityCommand::Source => "security source".to_owned(),
            SecurityCommand::Report(options) => {
                format!("security report {}", options.kind.as_str())
            }
            SecurityCommand::Deployment(options) => match options.phase {
                crate::cli::DeploymentPhase::Source => "security deployment source".to_owned(),
                crate::cli::DeploymentPhase::Image => "security deployment image".to_owned(),
            },
        };
        println!("开始 CI job：{name}（cargo xtask check ci {name}）");
        return super::security::run(command, plan);
    }
    let job = job_for_command(command)?;
    println!(
        "开始 CI job：{}（cargo xtask check ci {}）",
        job.definition().output_name,
        job.definition().command_name
    );
    let context = VerifyExecutionContext::new_ci(frontend_dir)?;
    let mut state = CheckExecutionState::default();
    for task in &plan.tasks {
        println!("开始 CI 原子任务：{}", task.id);
        execute_ci_task(task.executor, &context, &mut state)?;
    }
    Ok(())
}

fn execute_required_plan(options: &crate::cli::RequiredOptions, plan: &TaskPlan) -> Result<()> {
    let [task] = plan.tasks.as_slice() else {
        return Err("Required 计划必须仅包含一个汇总任务".into());
    };
    if task.executor != TaskExecutor::CiRequiredJobs {
        return Err("Required 计划包含非汇总任务".into());
    }
    println!("开始 CI 原子任务：{}", task.id);
    super::required::run(options)
}

fn execute_ci_task(
    executor: TaskExecutor,
    context: &VerifyExecutionContext,
    state: &mut CheckExecutionState,
) -> Result<()> {
    match executor {
        TaskExecutor::CiFrontendCheckout => {
            super::verify_frontend_checkout_from_environment(&context.frontend_dir)
        }
        TaskExecutor::CiContractSource => {
            super::verify_formal_contract_source_from_environment(&context.frontend_dir)
        }
        TaskExecutor::CiWindowsSmoke => super::windows_smoke(&context.frontend_dir),
        TaskExecutor::CiResourceGate => super::resource_gate::run(&context.frontend_dir),
        TaskExecutor::CiIntegration => super::integration(),
        executor => execute_registered_task(executor, TaskExecutionMode::Ci, context, state),
    }
}

fn job_for_command(command: &CiCommand) -> Result<CiJob> {
    match command {
        CiCommand::Preflight => Ok(CiJob::Preflight),
        CiCommand::RustGate => Ok(CiJob::RustGate),
        CiCommand::ResourceGate => Ok(CiJob::ResourceGate),
        CiCommand::Integration => Ok(CiJob::Integration),
        CiCommand::ConsumerContract => Ok(CiJob::ConsumerContract),
        CiCommand::Plan
        | CiCommand::ResourceGateReplay(_)
        | CiCommand::Required(_)
        | CiCommand::Security(_) => Err("该 CI 子命令不是独立 GitHub job 任务".into()),
    }
}

fn selected_jobs(selected: &[CiJob]) -> Result<Vec<CiJob>> {
    let jobs = CI_JOBS
        .iter()
        .filter_map(|definition| selected.contains(&definition.job).then_some(definition.job))
        .collect::<Vec<_>>();
    if jobs.len() != selected.len() {
        return Err("CI job 计划包含重复或未登记项".into());
    }
    Ok(jobs)
}
