use crate::{
    Result,
    check::{PYTHON_ENVIRONMENT_ARGS, TaskExecutor, TaskPlan},
    process::run,
    workspace::root_dir,
};

pub(super) const SOURCE_TASKS: &[TaskExecutor] = &[
    TaskExecutor::PythonEnvironment,
    TaskExecutor::CiSupplyChainSource,
    TaskExecutor::CiCargoAudit,
    TaskExecutor::CiCargoDeny,
];

pub(super) fn run_source(plan: &TaskPlan) -> Result<()> {
    let expected = TaskPlan::sequence(SOURCE_TASKS)?;
    if plan != &expected {
        return Err("Security source 计划与登记的原子任务不一致".into());
    }
    let root = root_dir();
    for task in &plan.tasks {
        println!("开始 CI 原子任务：{}", task.id);
        let (program, arguments) = source_command(task.executor)?;
        run(&root, program, arguments)?;
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
