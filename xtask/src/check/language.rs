//! 固定字节口径的语言构成报告；默认只读，阈值与新报告写入均须显式请求。

use std::path::{Component, Path, PathBuf};

use crate::{Result, process::PreservedFailure, workspace::root_dir};

use super::{TaskExecutor, TaskPlan, render_task_plan};

#[path = "language/git.rs"]
pub(crate) mod git;
#[path = "language/inventory.rs"]
pub(crate) mod inventory;
#[path = "language/model.rs"]
pub(crate) mod model;
#[path = "language/policy.rs"]
pub(crate) mod policy;
#[path = "language/report.rs"]
pub(crate) mod report;

pub(crate) use model::{LanguageOptions, Percent};

pub(crate) fn run(options: &LanguageOptions) -> Result<()> {
    let root = root_dir();
    run_with_git(&root, options, &mut |args| git::invoke(&root, args))
}

pub(crate) fn plan_at(root: &Path, options: &LanguageOptions) -> Result<TaskPlan> {
    resolve_output(root, options)?;
    let mut executors = vec![TaskExecutor::LanguageReport];
    if options.output.is_some() {
        executors.push(TaskExecutor::LanguageWrite);
    }
    if options.min_rust_percent.is_some() {
        executors.push(TaskExecutor::LanguageThreshold);
    }
    TaskPlan::sequence(&executors)
}

pub(crate) fn resolve_output(root: &Path, options: &LanguageOptions) -> Result<Option<PathBuf>> {
    if options.write != options.output.is_some() {
        return Err("--output 与 --write 必须同时指定；默认报告不写文件".into());
    }
    options
        .output
        .as_ref()
        .map(|value| {
            let path = if value.is_absolute() {
                value.clone()
            } else {
                root.join(value)
            };
            if value.as_os_str().is_empty()
                || value
                    .components()
                    .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
                || path
                    .to_str()
                    .is_none_or(|text| text.contains(['\n', '\r', '\0']))
                || path.extension().is_none_or(|extension| extension != "json")
                || path == root.join(".local-tests")
                || !path.starts_with(root.join(".local-tests"))
            {
                return Err(
                    "语言报告输出必须为当前后端 .local-tests 内的 JSON 新文件，且不得包含路径跳转"
                        .into(),
                );
            }
            Ok(path)
        })
        .transpose()
}

pub(crate) fn run_with_git(
    root: &Path,
    options: &LanguageOptions,
    git: &mut impl FnMut(&[&str]) -> Result<git::Reply>,
) -> Result<()> {
    let plan = plan_at(root, options)?;
    if options.plan {
        println!("语言报告计划（不执行 Git、不读取待统计文件、不创建目录或报告）：");
        render_task_plan(&plan);
        println!(
            "阈值={}；输出={}",
            options
                .min_rust_percent
                .map(Percent::text)
                .unwrap_or("未请求".into()),
            options
                .output
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or("仅 stdout".into())
        );
        return Ok(());
    }
    let output = resolve_output(root, options)?;
    let mut observation = None;
    for task in plan.tasks {
        match task.executor {
            TaskExecutor::LanguageReport => {
                if let Some(path) = &output {
                    report::validate_output(root, path, git)?;
                }
                let policy = policy::load()?;
                let inventory = inventory::discover(root, git)?;
                let files = inventory::collect(root, &policy, &inventory)?;
                if inventory::discover(root, git)? != inventory {
                    return Err("统计期间 Git 文件集合或 HEAD 变化，拒绝发布混合快照".into());
                }
                inventory::verify(root, &files)?;
                let result = report::build(policy, inventory, files, options.min_rust_percent)?;
                println!("{}", report::render(&result));
                observation = Some(result);
            }
            TaskExecutor::LanguageWrite => {
                let result = observation.as_ref().ok_or("报告写入缺少观测结果")?;
                let path = output.as_ref().ok_or("报告写入缺少输出路径")?;
                report::validate_output(root, path, git)?;
                report::write_new(root, path, result)?;
                println!("新报告已创建：{}", path.display());
            }
            TaskExecutor::LanguageThreshold => {
                let result = observation.as_ref().ok_or("阈值检查缺少观测结果")?;
                if result.threshold_passed != Some(true) {
                    return Err(PreservedFailure::new(
                        "自维护代码 Rust 字节占比未达到显式阈值；分母为空同样不通过".into(),
                        Some(1),
                    )
                    .into());
                }
            }
            _ => return Err("语言报告任务图含非语言任务".into()),
        }
    }
    Ok(())
}
