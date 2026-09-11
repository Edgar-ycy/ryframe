use std::path::Path;

use crate::{Result, cli::CheckScope, workspace::root_dir};

use super::{
    change_surface::{
        ChangeSurfaceReport, analyze_change_surface, append_changed_file_size_warnings,
        enforce_change_surface, load_change_surface_policy, print_change_surface,
    },
    model::{VerifySelection, WorkspaceGraph},
    selection::{
        changed_paths, classify_changes, complete_verify_selection, load_workspace_graph,
        print_selection,
    },
    task_plan::TaskPlan,
};

#[path = "plan_tasks.rs"]
mod tasks;

pub(crate) use tasks::{
    CheckTask, CheckTaskExecutor, CheckTaskRepository, CheckTaskStage, CheckTaskWorkingDirectory,
    tasks_for,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckPlanMode {
    ExplicitFull,
    ExpandedFull(String),
    Selected(VerifySelection),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CheckTaskPlan {
    pub(crate) surface: ChangeSurfaceReport,
    pub(crate) mode: CheckPlanMode,
    pub(crate) task_plan: TaskPlan<CheckTaskExecutor>,
}

pub(crate) fn plan(scope: CheckScope, full: bool, frontend_dir: &Path) -> Result<()> {
    let plan = build_task_plan(scope, full, &root_dir(), frontend_dir)?;
    validate_plan(&plan)?;
    render_plan(&plan);
    Ok(())
}

pub(crate) fn build_task_plan(
    scope: CheckScope,
    full: bool,
    root: &Path,
    frontend_dir: &Path,
) -> Result<CheckTaskPlan> {
    let backend_changes = changed_paths(root)?;
    let frontend_changes = changed_paths(frontend_dir)?;
    let policy = load_change_surface_policy(root)?;
    let mut surface = analyze_change_surface(&backend_changes, &frontend_changes, &policy);
    append_changed_file_size_warnings(
        root,
        frontend_dir,
        &backend_changes,
        &frontend_changes,
        &policy,
        &mut surface,
    )?;
    let graph = if !full && matches!(scope, CheckScope::All | CheckScope::Backend) {
        load_workspace_graph(root)?
    } else {
        WorkspaceGraph::default()
    };
    let mode = select_check_mode(scope, full, &backend_changes, &frontend_changes, &graph);
    let task_plan = TaskPlan::new(tasks_for(scope, &mode))?;
    Ok(CheckTaskPlan {
        surface,
        mode,
        task_plan,
    })
}

pub(crate) fn validate_plan(plan: &CheckTaskPlan) -> Result<()> {
    enforce_change_surface(&plan.surface)?;
    plan.task_plan.validate()
}

pub(crate) fn select_check_mode(
    scope: CheckScope,
    full: bool,
    backend_changes: &[String],
    frontend_changes: &[String],
    graph: &WorkspaceGraph,
) -> CheckPlanMode {
    if full {
        return CheckPlanMode::ExplicitFull;
    }
    let selected_backend = if matches!(scope, CheckScope::All | CheckScope::Backend) {
        backend_changes
    } else {
        &[]
    };
    let selected_frontend = if matches!(scope, CheckScope::All | CheckScope::Frontend) {
        frontend_changes
    } else {
        &[]
    };
    let mut selection = classify_changes(selected_backend, selected_frontend, graph);
    if let Some(reason) = selection.full_reason.take() {
        return CheckPlanMode::ExpandedFull(reason);
    }
    complete_verify_selection(&mut selection, graph);
    CheckPlanMode::Selected(selection)
}

pub(crate) fn render_plan(plan: &CheckTaskPlan) {
    print_change_surface(&plan.surface);
    match &plan.mode {
        CheckPlanMode::ExplicitFull => println!("任务图模式：完整（显式 --full）。"),
        CheckPlanMode::ExpandedFull(reason) => {
            println!("任务图扩大为完整门禁：{reason}");
        }
        CheckPlanMode::Selected(selection) => {
            print_selection(selection);
        }
    }
    if plan.task_plan.tasks.is_empty() {
        println!("任务图为空：当前范围仅有文档变更或没有变更。");
        return;
    }
    for task in &plan.task_plan.tasks {
        let dependencies = if task.dependencies.is_empty() {
            "无".to_owned()
        } else {
            task.dependencies.join(",")
        };
        println!(
            "任务 {}：仓库={}，阶段={}，依赖={}",
            task.id,
            task.repository.label(),
            task.stage.label(),
            dependencies
        );
        println!(
            "  工作目录={}；调用={}（{}）",
            task.working_directory.label(),
            task.executor.label(),
            task.executor.description()
        );
        if let Some(arguments) = task.executor.static_arguments() {
            println!("  固定参数={}", arguments.join(" "));
        }
        println!(
            "  编译覆盖={}；允许写入={}；外部资源={}",
            display_metadata(task.compilation_coverage),
            display_metadata(task.allowed_writes),
            if task.external_resources.is_empty() {
                "禁止".to_owned()
            } else {
                task.external_resources.join("、")
            }
        );
    }
}

fn display_metadata(values: &[&str]) -> String {
    if values.is_empty() {
        "无".to_owned()
    } else {
        values.join("、")
    }
}
