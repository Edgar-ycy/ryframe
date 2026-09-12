use crate::{
    Result,
    check::{PYTHON_ENVIRONMENT_ARGS, TaskExecutor, TaskPlan},
    cli::{SecurityCommand, SecurityReportKind, SecurityReportOptions},
    process::{run as run_process, run_owned},
    workspace::root_dir,
};

#[path = "security/deployment.rs"]
mod deployment;

#[allow(unused_imports)]
pub(crate) use deployment::{
    deployment_commands, deployment_required_at, plan_at, plan_for_required, run_at,
};

pub(super) const SOURCE_TASKS: &[TaskExecutor] = &[
    TaskExecutor::PythonEnvironment,
    TaskExecutor::CiSupplyChainSource,
    TaskExecutor::CiCargoAudit,
    TaskExecutor::CiCargoDeny,
];
const CYCLONEDX_REPORT_TASKS: &[TaskExecutor] = &[
    TaskExecutor::PythonEnvironment,
    TaskExecutor::CiCycloneDxReport,
];
const TRIVY_REPORT_TASKS: &[TaskExecutor] =
    &[TaskExecutor::PythonEnvironment, TaskExecutor::CiTrivyReport];

pub(super) fn plan(command: &SecurityCommand) -> Result<TaskPlan> {
    match command {
        SecurityCommand::Source => TaskPlan::sequence(SOURCE_TASKS),
        SecurityCommand::Report(options) => {
            validate_report_options(options)?;
            TaskPlan::sequence(report_tasks(options.kind))
        }
        SecurityCommand::Deployment(options) => deployment::plan(options),
    }
}

pub(super) fn run(command: &SecurityCommand, plan: &TaskPlan) -> Result<()> {
    match command {
        SecurityCommand::Source => run_source(plan),
        SecurityCommand::Report(options) => run_report(options, plan),
        SecurityCommand::Deployment(options) => deployment::run(options, plan),
    }
}

fn report_tasks(kind: SecurityReportKind) -> &'static [TaskExecutor] {
    match kind {
        SecurityReportKind::CycloneDx => CYCLONEDX_REPORT_TASKS,
        SecurityReportKind::Trivy => TRIVY_REPORT_TASKS,
    }
}

fn validate_report_options(options: &SecurityReportOptions) -> Result<()> {
    if !options.input.is_absolute() {
        return Err("Security report 输入必须是绝对路径".into());
    }
    if options.kind == SecurityReportKind::Trivy && options.require_reproducible {
        return Err("Trivy 报告不支持 --require-reproducible".into());
    }
    Ok(())
}

fn run_report(options: &SecurityReportOptions, plan: &TaskPlan) -> Result<()> {
    validate_report_options(options)?;
    let expected = TaskPlan::sequence(report_tasks(options.kind))?;
    if plan != &expected {
        return Err("Security report 计划与登记的原子任务不一致".into());
    }
    let root = root_dir();
    for task in &plan.tasks {
        println!("开始 CI 原子任务：{}", task.id);
        let (program, arguments) = report_command(task.executor, options)?;
        run_owned(&root, program, &arguments)?;
    }
    Ok(())
}

fn run_source(plan: &TaskPlan) -> Result<()> {
    let expected = TaskPlan::sequence(SOURCE_TASKS)?;
    if plan != &expected {
        return Err("Security source 计划与登记的原子任务不一致".into());
    }
    let root = root_dir();
    for task in &plan.tasks {
        println!("开始 CI 原子任务：{}", task.id);
        let (program, arguments) = source_command(task.executor)?;
        run_process(&root, program, arguments)?;
    }
    Ok(())
}

pub(crate) fn source_command(
    executor: TaskExecutor,
) -> Result<(&'static str, &'static [&'static str])> {
    let program = match executor {
        TaskExecutor::PythonEnvironment | TaskExecutor::CiSupplyChainSource => "python",
        TaskExecutor::CiCargoAudit | TaskExecutor::CiCargoDeny => "cargo",
        _ => return Err("非 Security source 节点不能由安全执行器运行".into()),
    };
    let arguments = if executor == TaskExecutor::PythonEnvironment {
        PYTHON_ENVIRONMENT_ARGS
    } else {
        executor
            .static_arguments()
            .ok_or("Security source 节点缺少固定参数")?
    };
    Ok((program, arguments))
}

pub(crate) fn report_command(
    executor: TaskExecutor,
    options: &SecurityReportOptions,
) -> Result<(&'static str, Vec<String>)> {
    validate_report_options(options)?;
    if executor == TaskExecutor::PythonEnvironment {
        return Ok((
            "python",
            PYTHON_ENVIRONMENT_ARGS
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
        ));
    }
    let expected = match options.kind {
        SecurityReportKind::CycloneDx => TaskExecutor::CiCycloneDxReport,
        SecurityReportKind::Trivy => TaskExecutor::CiTrivyReport,
    };
    if executor != expected {
        return Err("报告类型与 Security report 任务节点不一致".into());
    }
    let input = options
        .input
        .to_str()
        .ok_or("Security report 输入路径必须能表示为 UTF-8")?;
    let report_option = match options.kind {
        SecurityReportKind::CycloneDx => "--cyclonedx",
        SecurityReportKind::Trivy => "--trivy-report",
    };
    let mut arguments = vec![
        "scripts/check_supply_chain.py".to_owned(),
        report_option.to_owned(),
        input.to_owned(),
    ];
    if options.require_reproducible {
        arguments.push("--require-reproducible-cyclonedx".to_owned());
    }
    Ok(("python", arguments))
}
