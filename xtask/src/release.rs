use std::path::Path;

use crate::{
    Result,
    check::{PYTHON_ENVIRONMENT_ARGS, TaskExecutor, TaskPlan, render_task_plan},
    cli::{
        ReleaseCiEvidenceOptions, ReleaseCiOperation, ReleaseCiOptions, ReleaseCommand,
        ReleaseSourceOptions,
    },
    process::{run as run_process, run_owned},
    workspace::root_dir,
};

const SOURCE_TASKS: &[TaskExecutor] =
    &[TaskExecutor::PythonEnvironment, TaskExecutor::ReleaseSource];
const CI_TASKS: &[TaskExecutor] = &[TaskExecutor::PythonEnvironment, TaskExecutor::ReleaseCi];

pub(crate) fn run(command: &ReleaseCommand, frontend_dir: &Path) -> Result<()> {
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
            TaskExecutor::PythonEnvironment => {
                run_process(&root_dir(), "python", PYTHON_ENVIRONMENT_ARGS)?;
            }
            TaskExecutor::ReleaseSource => {
                let ReleaseCommand::Source(options) = command else {
                    return Err("发布来源任务收到错误的命令类型".into());
                };
                run_owned(
                    &root_dir(),
                    "python",
                    &source_arguments(options, frontend_dir)?,
                )?;
            }
            TaskExecutor::ReleaseCi => {
                let ReleaseCommand::Ci(options) = command else {
                    return Err("发布 CI 任务收到错误的命令类型".into());
                };
                run_owned(&root_dir(), "python", &ci_arguments(options, frontend_dir)?)?;
            }
            _ => return Err("发布核验计划包含非发布任务".into()),
        }
    }
    Ok(())
}

pub(crate) fn source_arguments(
    options: &ReleaseSourceOptions,
    frontend_dir: &Path,
) -> Result<Vec<String>> {
    validate_absolute(&options.manifest_path, "发布清单")?;
    let frontend = canonical_utf8(frontend_dir, "前端目录")?;
    Ok(vec![
        "scripts/validate_release.py".to_owned(),
        "--tag".to_owned(),
        options.tag.as_str().to_owned(),
        "--frontend-dir".to_owned(),
        frontend,
        "--backend-repository".to_owned(),
        options.backend_repository.as_str().to_owned(),
        "--backend-commit".to_owned(),
        options.backend_commit.as_str().to_owned(),
        "--frontend-repository".to_owned(),
        options.frontend_repository.as_str().to_owned(),
        "--frontend-commit".to_owned(),
        options.frontend_commit.as_str().to_owned(),
        "--manifest-path".to_owned(),
        utf8(&options.manifest_path, "发布清单")?,
    ])
}

pub(crate) fn ci_arguments(options: &ReleaseCiOptions, frontend_dir: &Path) -> Result<Vec<String>> {
    let mut arguments = vec!["scripts/verify_release_ci.py".to_owned()];
    match &options.operation {
        ReleaseCiOperation::Evidence(evidence) => {
            append_evidence_arguments(&mut arguments, evidence)?;
        }
        ReleaseCiOperation::RecordPair { output } => {
            append_pair_arguments(&mut arguments, "--record-pair", output, frontend_dir)?;
        }
        ReleaseCiOperation::VerifyPair { input } => {
            append_pair_arguments(&mut arguments, "--verify-pair", input, frontend_dir)?;
        }
    }
    Ok(arguments)
}

fn append_evidence_arguments(
    arguments: &mut Vec<String>,
    options: &ReleaseCiEvidenceOptions,
) -> Result<()> {
    validate_absolute(&options.output, "发布 CI 证据")?;
    append_value(
        arguments,
        "--backend-repository",
        options.backend_repository.as_str(),
    );
    append_value(
        arguments,
        "--frontend-repository",
        options.frontend_repository.as_str(),
    );
    append_value(arguments, "--backend-sha", options.backend_sha.as_str());
    append_value(arguments, "--frontend-sha", options.frontend_sha.as_str());
    if let Some(tag_oids) = &options.tag_oids {
        append_value(arguments, "--backend-tag-oid", tag_oids.backend.as_str());
        append_value(arguments, "--frontend-tag-oid", tag_oids.frontend.as_str());
    }
    append_value(arguments, "--tag", options.tag.as_str());
    append_value(arguments, "--timeout", &options.timeout_seconds.to_string());
    append_value(
        arguments,
        "--output",
        &utf8(&options.output, "发布 CI 证据")?,
    );
    Ok(())
}

fn append_pair_arguments(
    arguments: &mut Vec<String>,
    option: &str,
    receipt: &Path,
    frontend_dir: &Path,
) -> Result<()> {
    validate_absolute(receipt, "源码组合收据")?;
    append_value(arguments, option, &utf8(receipt, "源码组合收据")?);
    append_value(
        arguments,
        "--backend-dir",
        &canonical_utf8(&root_dir(), "后端目录")?,
    );
    append_value(
        arguments,
        "--frontend-dir",
        &canonical_utf8(frontend_dir, "前端目录")?,
    );
    Ok(())
}

fn append_value(arguments: &mut Vec<String>, option: &str, value: &str) {
    arguments.extend([option.to_owned(), value.to_owned()]);
}

fn validate_command(command: &ReleaseCommand) -> Result<()> {
    match command {
        ReleaseCommand::Source(options) => validate_absolute(&options.manifest_path, "发布清单"),
        ReleaseCommand::Ci(options) => match &options.operation {
            ReleaseCiOperation::Evidence(options) => {
                validate_absolute(&options.output, "发布 CI 证据")
            }
            ReleaseCiOperation::RecordPair { output } => validate_absolute(output, "源码组合收据"),
            ReleaseCiOperation::VerifyPair { input } => validate_absolute(input, "源码组合收据"),
        },
    }
}

fn validate_absolute(path: &Path, label: &str) -> Result<()> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(format!("{label}必须是绝对路径").into())
    }
}

fn canonical_utf8(path: &Path, label: &str) -> Result<String> {
    let path = path
        .canonicalize()
        .map_err(|error| format!("无法解析{label}：{error}"))?;
    utf8(&path, label)
}

fn utf8(path: &Path, label: &str) -> Result<String> {
    path.to_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{label}路径不是有效 UTF-8").into())
}

fn command_label(command: &ReleaseCommand) -> &'static str {
    match command {
        ReleaseCommand::Source(_) => "source",
        ReleaseCommand::Ci(_) => "ci",
    }
}
