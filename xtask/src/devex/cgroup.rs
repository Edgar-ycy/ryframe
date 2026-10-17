use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

use crate::{Result, check::TaskPlan, workspace::root_dir};

use super::{DevexCgroupOperation, DevexCgroupOptions};

#[path = "cgroup/error.rs"]
mod error;
#[path = "cgroup/evidence.rs"]
mod evidence;
#[cfg(target_os = "linux")]
#[path = "cgroup/linux.rs"]
mod linux;
#[path = "cgroup/plan.rs"]
mod plan;
#[path = "cgroup/processes.rs"]
mod processes;
#[cfg(target_os = "linux")]
#[path = "cgroup/stage.rs"]
mod stage;

pub(crate) const STAGE_MARKER: &str = "__ryframe_devex_cgroup_stage";
pub(crate) const RUNNER_MARKER: &str = "__ryframe_devex_cgroup_runner";

#[allow(unused_imports)]
pub(crate) use error::GateError;
#[allow(unused_imports)]
pub(crate) use evidence::{
    CgroupPhase, CleanupPreflight, CleanupResult, CleanupStatus, CreatedReceipt, KernelIdentity,
    ParentState, inspect_cleanup, parent_state_from, parse_events, read_cleanup_result, read_json,
    write_failure, write_json_new,
};
#[allow(unused_imports)]
pub(crate) use plan::{
    CGROUP_ROOT, CgroupPlan, TEST_CASE, cargo_command, privileged_identity_matches,
    validate_plan_at, validate_test_output,
};
#[allow(unused_imports)]
pub(crate) use processes::{
    ProcessFact, ProcessRegistry, ProcessSnapshot, append_registry, create_registry, read_registry,
    register_children, registry_path, verify_processes,
};

pub(super) fn validate_cli_options(
    options: &DevexCgroupOptions,
) -> std::result::Result<(), String> {
    resolve_output_at(&options.output, &root_dir())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(crate) fn run(options: &DevexCgroupOptions) -> Result<()> {
    let root = root_dir();
    let output = resolve_output_at(&options.output, &root)?;
    let task_plan = plan_at(options, &root)?;
    if options.plan {
        render_plan(&task_plan, options.operation);
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        Ok(linux::run_public(
            options.operation,
            &output,
            &root,
            &task_plan,
        )?)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = output;
        Err(
            "DevEx cgroup 门禁只允许在 Linux GitHub Actions 的非 root runner 中运行；当前平台不支持真实 cgroup v2 验收。请在 Extended CI 或具备 cgroup v2 memory controller 的 Linux 隔离环境执行。"
                .into(),
        )
    }
}

pub(crate) fn plan_at(options: &DevexCgroupOptions, root: &Path) -> Result<TaskPlan> {
    resolve_output_at(&options.output, root)?;
    let executor = match options.operation {
        DevexCgroupOperation::Run => crate::check::TaskExecutor::PerfCgroupRun,
        DevexCgroupOperation::Cleanup => crate::check::TaskExecutor::PerfCgroupCleanup,
    };
    TaskPlan::sequence(&[executor])
}

pub(crate) fn run_hidden_if_requested() -> Option<Result<()>> {
    let mut arguments = env::args_os().skip(1);
    let marker = arguments.next()?;
    if marker != STAGE_MARKER && marker != RUNNER_MARKER {
        return None;
    }
    let remaining = arguments.collect::<Vec<_>>();
    #[cfg(target_os = "linux")]
    {
        let result = if marker == STAGE_MARKER {
            stage::run_cli(&remaining)
        } else {
            linux::run_runner_cli(&remaining)
        };
        Some(result.map_err(Into::into))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = remaining;
        Some(Err("DevEx cgroup 内部阶段只允许在 Linux 运行".into()))
    }
}

pub(crate) fn resolve_output_at(value: &Path, root: &Path) -> Result<PathBuf> {
    if value.as_os_str().is_empty() || value.components().any(|part| part == Component::ParentDir) {
        return Err("DevEx cgroup 输出目录不得为空或包含父目录跳转".into());
    }
    let output = if value.is_absolute() {
        value.to_path_buf()
    } else {
        root.join(value)
    };
    let local_tests = root.join(".local-tests");
    if output == local_tests || !output.starts_with(&local_tests) {
        return Err("DevEx cgroup 输出必须位于当前后端 .local-tests 的子目录".into());
    }
    validate_existing_components(&output, &local_tests)?;
    Ok(output)
}

fn validate_existing_components(output: &Path, boundary: &Path) -> Result<()> {
    let mut current = Some(output);
    while let Some(path) = current {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("DevEx cgroup 输出路径不能经过符号链接".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(
                    format!("无法核验 DevEx cgroup 输出路径 {}：{error}", path.display()).into(),
                );
            }
        }
        if path == boundary {
            break;
        }
        current = path.parent();
    }
    Ok(())
}

fn render_plan(plan: &TaskPlan, operation: DevexCgroupOperation) {
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
    let stages = match operation {
        DevexCgroupOperation::Run => "prepare → test → cleanup（Rust xtask 内部按权限边界执行）",
        DevexCgroupOperation::Cleanup => "cleanup（Rust xtask 精确清理）",
    };
    println!("  执行：{stages}");
}
