use std::path::Path;

use crate::{Result, cli::CheckScope, workspace::root_dir};

use super::{
    change_surface::{
        analyze_change_surface, append_changed_file_size_warnings, enforce_change_surface,
        load_change_surface_policy, print_change_surface,
    },
    model::WorkspaceGraph,
    selection::{
        changed_paths, classify_changes, complete_verify_selection, load_workspace_graph,
        print_selection,
    },
};

pub(crate) fn plan(scope: CheckScope, full: bool, frontend_dir: &Path) -> Result<()> {
    let root = root_dir();
    let backend_changes = changed_paths(&root)?;
    let frontend_changes = changed_paths(frontend_dir)?;
    let policy = load_change_surface_policy(&root)?;
    let mut surface = analyze_change_surface(&backend_changes, &frontend_changes, &policy);
    append_changed_file_size_warnings(
        &root,
        frontend_dir,
        &backend_changes,
        &frontend_changes,
        &policy,
        &mut surface,
    )?;
    print_change_surface(&surface);
    enforce_change_surface(&surface)?;
    if full {
        print_full_plan();
        return Ok(());
    }
    let graph = if matches!(scope, CheckScope::All | CheckScope::Backend) {
        load_workspace_graph(&root)?
    } else {
        WorkspaceGraph::default()
    };
    let selected_backend = if matches!(scope, CheckScope::All | CheckScope::Backend) {
        backend_changes.as_slice()
    } else {
        &[]
    };
    let selected_frontend = if matches!(scope, CheckScope::All | CheckScope::Frontend) {
        frontend_changes.as_slice()
    } else {
        &[]
    };
    let mut selection = classify_changes(selected_backend, selected_frontend, &graph);
    if let Some(reason) = &selection.full_reason {
        println!("任务图扩大为完整门禁：{reason}");
        print_full_dependencies();
        return Ok(());
    }
    complete_verify_selection(&mut selection, &graph);
    print_selection(&selection);
    println!(
        "任务依赖与去重：Cargo 包检查复用同一 target；快照先于消费契约；前端完整单测最多执行一次。"
    );
    Ok(())
}

fn print_full_plan() {
    println!(
        "完整任务图：后端基础检查与前端静态检查并行；随后执行 Rust 测试、资源切片、契约消费、覆盖率和生产构建。浏览器验收单独执行。"
    );
    println!(
        "编译覆盖：Workspace Clippy、Workspace test、feature matrix、资源生成器默认与 schema-import feature。"
    );
}

fn print_full_dependencies() {
    println!("依赖顺序：基础静态检查 → 编译与测试 → 快照、消费契约与生产构建。");
}
