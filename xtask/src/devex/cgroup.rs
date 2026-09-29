use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use crate::{
    Result,
    check::{TaskExecutor, TaskPlan},
    process::run_owned,
    workspace::root_dir,
};

use super::{DevexCgroupOperation, DevexCgroupOptions};

const SCRIPT: &str = "scripts/ci_devex_cgroup.py";

pub(super) fn validate_cli_options(
    options: &DevexCgroupOptions,
) -> std::result::Result<(), String> {
    resolve_output_at(&options.output, &root_dir()).map(|_| ())
}

pub(super) fn run(options: &DevexCgroupOptions) -> Result<()> {
    let root = root_dir();
    let plan = plan_at(options, &root)?;
    let (program, arguments) = command_at(options, &plan, &root)?;
    if options.plan {
        render_plan(&plan, program, &arguments);
        return Ok(());
    }
    run_owned(&root, program, &arguments)
}

pub(crate) fn plan_at(options: &DevexCgroupOptions, root: &Path) -> Result<TaskPlan> {
    resolve_output_at(&options.output, root)?;
    TaskPlan::sequence(&[executor(options.operation)])
}

pub(crate) fn command_at(
    options: &DevexCgroupOptions,
    plan: &TaskPlan,
    root: &Path,
) -> Result<(&'static str, Vec<String>)> {
    let expected = plan_at(options, root)?;
    if plan != &expected {
        return Err("DevEx cgroup 计划与登记任务不一致".into());
    }
    let output = resolve_output_at(&options.output, root)?;
    let output = output
        .to_str()
        .ok_or("DevEx cgroup 输出路径必须能表示为 UTF-8")?;
    Ok((
        "python",
        vec![
            "-B".to_owned(),
            SCRIPT.to_owned(),
            options.operation.as_str().to_owned(),
            "--output".to_owned(),
            output.to_owned(),
        ],
    ))
}

fn executor(operation: DevexCgroupOperation) -> TaskExecutor {
    match operation {
        DevexCgroupOperation::Run => TaskExecutor::PerfCgroupRun,
        DevexCgroupOperation::Cleanup => TaskExecutor::PerfCgroupCleanup,
    }
}

fn resolve_output_at(value: &Path, root: &Path) -> std::result::Result<PathBuf, String> {
    if value.as_os_str().is_empty() || value.components().any(|part| part == Component::ParentDir) {
        return Err("DevEx cgroup 输出目录不得为空或包含父目录跳转".to_owned());
    }
    let output = if value.is_absolute() {
        value.to_path_buf()
    } else {
        root.join(value)
    };
    let local_tests = root.join(".local-tests");
    if output == local_tests || !output.starts_with(&local_tests) {
        return Err("DevEx cgroup 输出必须位于当前后端 .local-tests 的子目录".to_owned());
    }
    validate_existing_components(&output, &local_tests)?;
    Ok(output)
}

fn validate_existing_components(output: &Path, boundary: &Path) -> std::result::Result<(), String> {
    let mut current = Some(output);
    while let Some(path) = current {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("DevEx cgroup 输出路径不能经过符号链接".to_owned());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "无法核验 DevEx cgroup 输出路径 {}：{error}",
                    path.display()
                ));
            }
        }
        if path == boundary {
            break;
        }
        current = path.parent();
    }
    Ok(())
}

fn render_plan(plan: &TaskPlan, program: &str, arguments: &[String]) {
    println!("DevEx cgroup 计划（只读）：");
    for task in &plan.tasks {
        let definition = task.definition();
        println!(
            "- {} [{}]：{}",
            definition.id,
            definition.stage.label(),
            definition.description
        );
        println!("  工作目录：{}", definition.working_directory.label());
        println!("  写入范围：{}", definition.allowed_writes.join("、"));
        println!("  外部资源：{}", definition.external_resources.join("、"));
    }
    println!("  执行：{program} {}", arguments.join(" "));
}
