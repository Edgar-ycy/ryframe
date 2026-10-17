use std::path::Path;

use crate::{
    Result,
    check::{TaskExecutor, TaskPlan, render_task_plan},
    cli::{ReleaseCiOperation, ReleaseCommand},
};

#[path = "release/archive.rs"]
pub(crate) mod archive;
#[path = "release/evidence.rs"]
pub(crate) mod evidence;
#[path = "release/github.rs"]
pub(crate) mod github;
#[path = "release/ci.rs"]
pub(crate) mod release_ci;
#[path = "release/source.rs"]
pub(crate) mod source;
#[path = "release/support.rs"]
pub(crate) mod support;

const SOURCE_TASKS: &[TaskExecutor] = &[TaskExecutor::ReleaseSource];
const CI_TASKS: &[TaskExecutor] = &[TaskExecutor::ReleaseCi];

pub(crate) fn run(command: &ReleaseCommand, frontend_dir: &Path) -> Result<()> {
    support::validate_text_path(frontend_dir, "前端目录")?;
    let plan = plan(command)?;
    if command.plan() {
        println!("发布核验计划：{}", command_label(command));
        render_task_plan(&plan);
        return Ok(());
    }
    execute(command, &plan, frontend_dir)
}

pub(crate) fn plan(command: &ReleaseCommand) -> Result<TaskPlan> {
    validate_command(command)?;
    TaskPlan::sequence(match command {
        ReleaseCommand::Source(_) => SOURCE_TASKS,
        ReleaseCommand::Ci(_) => CI_TASKS,
    })
}

fn execute(command: &ReleaseCommand, plan: &TaskPlan, frontend_dir: &Path) -> Result<()> {
    let expected = self::plan(command)?;
    if plan != &expected {
        return Err("发布核验计划与登记的原子任务不一致".into());
    }
    for task in &plan.tasks {
        println!("开始发布核验原子任务：{}", task.id);
        match task.executor {
            TaskExecutor::ReleaseSource => {
                let ReleaseCommand::Source(options) = command else {
                    return Err("发布来源任务收到错误的命令类型".into());
                };
                source::validate(options, frontend_dir)?;
            }
            TaskExecutor::ReleaseCi => {
                let ReleaseCommand::Ci(options) = command else {
                    return Err("发布 CI 任务收到错误的命令类型".into());
                };
                release_ci::run(options, frontend_dir)?;
            }
            _ => return Err("发布核验计划包含非发布任务".into()),
        }
    }
    Ok(())
}

fn validate_command(command: &ReleaseCommand) -> Result<()> {
    match command {
        ReleaseCommand::Source(options) => {
            support::validate_absolute(&options.manifest_path, "发布清单")
        }
        ReleaseCommand::Ci(options) => match &options.operation {
            ReleaseCiOperation::Evidence(options) => {
                support::validate_absolute(&options.output, "发布 CI 证据")
            }
            ReleaseCiOperation::RecordPair { output } => {
                support::validate_absolute(output, "源码组合收据")
            }
            ReleaseCiOperation::VerifyPair { input } => {
                support::validate_absolute(input, "源码组合收据")
            }
        },
    }
}

fn command_label(command: &ReleaseCommand) -> &'static str {
    match command {
        ReleaseCommand::Source(_) => "source",
        ReleaseCommand::Ci(_) => "ci",
    }
}
