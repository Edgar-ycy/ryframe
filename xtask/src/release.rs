use std::path::Path;

use crate::{
    Result,
    check::{PYTHON_ENVIRONMENT_ARGS, TaskExecutor, TaskPlan, render_task_plan},
    cli::{
        ReleaseCiEvidenceOptions, ReleaseCiOperation, ReleaseCiOptions, ReleaseCommand,
        ReleaseSourceOptions,
    },
    process::{run as run_process, run_with_env_removed},
    workspace::root_dir,
};

const SOURCE_TASKS: &[TaskExecutor] =
    &[TaskExecutor::PythonEnvironment, TaskExecutor::ReleaseSource];
const CI_TASKS: &[TaskExecutor] = &[TaskExecutor::PythonEnvironment, TaskExecutor::ReleaseCi];

const PROTOCOL_VERSION: &str = "1";
const VERSION_KEY: &str = "RYFRAME_RELEASE_PROTOCOL_VERSION";
const MODE_KEY: &str = "RYFRAME_RELEASE_MODE";
pub(crate) const PROTOCOL_KEYS: &[&str] = &[
    VERSION_KEY,
    MODE_KEY,
    "RYFRAME_RELEASE_TAG",
    "RYFRAME_RELEASE_FRONTEND_DIR",
    "RYFRAME_RELEASE_BACKEND_DIR",
    "RYFRAME_RELEASE_BACKEND_REPOSITORY",
    "RYFRAME_RELEASE_BACKEND_COMMIT",
    "RYFRAME_RELEASE_FRONTEND_REPOSITORY",
    "RYFRAME_RELEASE_FRONTEND_COMMIT",
    "RYFRAME_RELEASE_MANIFEST_PATH",
    "RYFRAME_RELEASE_BACKEND_SHA",
    "RYFRAME_RELEASE_FRONTEND_SHA",
    "RYFRAME_RELEASE_BACKEND_TAG_OID",
    "RYFRAME_RELEASE_FRONTEND_TAG_OID",
    "RYFRAME_RELEASE_TIMEOUT_SECONDS",
    "RYFRAME_RELEASE_OUTPUT_PATH",
    "RYFRAME_RELEASE_INPUT_PATH",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivatePythonInvocation {
    pub(crate) script: &'static str,
    pub(crate) environment: Vec<(&'static str, String)>,
}

pub(crate) fn run(command: &ReleaseCommand, frontend_dir: &Path) -> Result<()> {
    validate_protocol_path(frontend_dir, "前端目录")?;
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
                run_private(source_invocation(options, frontend_dir)?)?;
            }
            TaskExecutor::ReleaseCi => {
                let ReleaseCommand::Ci(options) = command else {
                    return Err("发布 CI 任务收到错误的命令类型".into());
                };
                run_private(ci_invocation(options, frontend_dir)?)?;
            }
            _ => return Err("发布核验计划包含非发布任务".into()),
        }
    }
    Ok(())
}

fn run_private(invocation: PrivatePythonInvocation) -> Result<()> {
    let environment = invocation
        .environment
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    run_with_env_removed(
        &root_dir(),
        "python",
        &[invocation.script],
        &environment,
        PROTOCOL_KEYS,
    )
}

pub(crate) fn source_invocation(
    options: &ReleaseSourceOptions,
    frontend_dir: &Path,
) -> Result<PrivatePythonInvocation> {
    validate_absolute(&options.manifest_path, "发布清单")?;
    let mut environment = protocol_environment("source");
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_TAG",
        options.tag.as_str(),
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_FRONTEND_DIR",
        &canonical_utf8(frontend_dir, "前端目录")?,
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_BACKEND_REPOSITORY",
        options.backend_repository.as_str(),
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_BACKEND_COMMIT",
        options.backend_commit.as_str(),
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_FRONTEND_REPOSITORY",
        options.frontend_repository.as_str(),
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_FRONTEND_COMMIT",
        options.frontend_commit.as_str(),
    )?;
    push_path(
        &mut environment,
        "RYFRAME_RELEASE_MANIFEST_PATH",
        &options.manifest_path,
        "发布清单",
    )?;
    Ok(PrivatePythonInvocation {
        script: "scripts/validate_release.py",
        environment,
    })
}

pub(crate) fn ci_invocation(
    options: &ReleaseCiOptions,
    frontend_dir: &Path,
) -> Result<PrivatePythonInvocation> {
    let environment = match &options.operation {
        ReleaseCiOperation::Evidence(evidence) => evidence_environment(evidence)?,
        ReleaseCiOperation::RecordPair { output } => pair_environment(
            "ci-record-pair",
            "RYFRAME_RELEASE_OUTPUT_PATH",
            output,
            frontend_dir,
        )?,
        ReleaseCiOperation::VerifyPair { input } => pair_environment(
            "ci-verify-pair",
            "RYFRAME_RELEASE_INPUT_PATH",
            input,
            frontend_dir,
        )?,
    };
    Ok(PrivatePythonInvocation {
        script: "scripts/verify_release_ci.py",
        environment,
    })
}

fn evidence_environment(options: &ReleaseCiEvidenceOptions) -> Result<Vec<(&'static str, String)>> {
    validate_absolute(&options.output, "发布 CI 证据")?;
    let mut environment = protocol_environment("ci-evidence");
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_BACKEND_REPOSITORY",
        options.backend_repository.as_str(),
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_FRONTEND_REPOSITORY",
        options.frontend_repository.as_str(),
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_BACKEND_SHA",
        options.backend_sha.as_str(),
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_FRONTEND_SHA",
        options.frontend_sha.as_str(),
    )?;
    if let Some(tag_oids) = &options.tag_oids {
        push_value(
            &mut environment,
            "RYFRAME_RELEASE_BACKEND_TAG_OID",
            tag_oids.backend.as_str(),
        )?;
        push_value(
            &mut environment,
            "RYFRAME_RELEASE_FRONTEND_TAG_OID",
            tag_oids.frontend.as_str(),
        )?;
    }
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_TAG",
        options.tag.as_str(),
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_TIMEOUT_SECONDS",
        &options.timeout_seconds.to_string(),
    )?;
    push_path(
        &mut environment,
        "RYFRAME_RELEASE_OUTPUT_PATH",
        &options.output,
        "发布 CI 证据",
    )?;
    Ok(environment)
}

fn pair_environment(
    mode: &'static str,
    receipt_key: &'static str,
    receipt: &Path,
    frontend_dir: &Path,
) -> Result<Vec<(&'static str, String)>> {
    validate_absolute(receipt, "源码组合收据")?;
    let mut environment = protocol_environment(mode);
    push_path(&mut environment, receipt_key, receipt, "源码组合收据")?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_BACKEND_DIR",
        &canonical_utf8(&root_dir(), "后端目录")?,
    )?;
    push_value(
        &mut environment,
        "RYFRAME_RELEASE_FRONTEND_DIR",
        &canonical_utf8(frontend_dir, "前端目录")?,
    )?;
    Ok(environment)
}

fn protocol_environment(mode: &'static str) -> Vec<(&'static str, String)> {
    vec![
        (VERSION_KEY, PROTOCOL_VERSION.to_owned()),
        (MODE_KEY, mode.to_owned()),
    ]
}

fn push_path(
    environment: &mut Vec<(&'static str, String)>,
    key: &'static str,
    path: &Path,
    label: &str,
) -> Result<()> {
    push_value(environment, key, &utf8(path, label)?)
}

fn push_value(
    environment: &mut Vec<(&'static str, String)>,
    key: &'static str,
    value: &str,
) -> Result<()> {
    validate_protocol_value(value, key)?;
    environment.push((key, value.to_owned()));
    Ok(())
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
    if !path.is_absolute() {
        return Err(format!("{label}必须是绝对路径").into());
    }
    validate_protocol_path(path, label)
}

fn validate_protocol_path(path: &Path, label: &str) -> Result<()> {
    let value = utf8(path, label)?;
    validate_protocol_value(&value, label)
}

fn validate_protocol_value(value: &str, label: &str) -> Result<()> {
    if value.is_empty() {
        return Err(format!("{label}不能为空").into());
    }
    if value.contains(['\r', '\n', '\0']) {
        return Err(format!("{label}不能包含换行符或 NUL").into());
    }
    Ok(())
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
