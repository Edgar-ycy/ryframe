use std::collections::{BTreeMap, BTreeSet};

use super::{
    check::{
        CheckPlanMode, CheckTaskExecutor, CheckTaskRepository, CheckTaskStage,
        CheckTaskWorkingDirectory, FrontendProfile, WorkspaceGraph, select_check_mode, tasks_for,
    },
    cli::CheckScope,
};

fn task_ids(mode: &CheckPlanMode, scope: CheckScope) -> Vec<&'static str> {
    tasks_for(scope, mode)
        .into_iter()
        .map(|task| task.id)
        .collect()
}

#[test]
fn full_all_plan_exposes_the_executed_topology_and_metadata() {
    let tasks = tasks_for(CheckScope::All, &CheckPlanMode::ExplicitFull);
    assert_eq!(
        tasks.iter().map(|task| task.id).collect::<Vec<_>>(),
        [
            "full.python-environment",
            "full.frontend-dependencies",
            "full.backend-static",
            "full.snapshots-prepare",
            "full.python-tests",
            "full.migration-history",
            "full.workspaces",
            "full.snapshots-verify",
            "full.frontend",
        ]
    );
    let mut seen = BTreeSet::new();
    for task in &tasks {
        assert!(
            task.dependencies
                .iter()
                .all(|dependency| seen.contains(dependency))
        );
        assert!(!task.executor.label().is_empty());
        assert!(!task.executor.description().is_empty());
        assert!(task.external_resources.is_empty());
        seen.insert(task.id);
    }
    let workspaces = tasks
        .iter()
        .find(|task| task.id == "full.workspaces")
        .unwrap();
    assert_eq!(workspaces.repository, CheckTaskRepository::CrossRepository);
    assert_eq!(workspaces.stage, CheckTaskStage::Test);
    assert_eq!(
        workspaces.working_directory,
        CheckTaskWorkingDirectory::Backend
    );
    assert_eq!(workspaces.executor, CheckTaskExecutor::FullWorkspaces);
    assert!(workspaces.compilation_coverage.contains(&"feature matrix"));
    assert!(workspaces.allowed_writes.contains(&"并行任务日志"));
    let python = tasks
        .iter()
        .find(|task| task.id == "full.python-environment")
        .unwrap();
    assert_eq!(python.repository, CheckTaskRepository::Backend);
    assert_eq!(python.stage, CheckTaskStage::Prerequisite);
    assert_eq!(python.executor, CheckTaskExecutor::FullPythonEnvironment);
    assert!(python.dependencies.is_empty());
}

#[test]
fn full_scope_keeps_backend_contract_and_frontend_only_paths_distinct() {
    let backend = tasks_for(CheckScope::Backend, &CheckPlanMode::ExplicitFull);
    assert_eq!(backend.last().unwrap().id, "full.consumer-contract");
    assert_eq!(
        backend.last().unwrap().executor,
        CheckTaskExecutor::FullBackendConsumerContract
    );

    let frontend = tasks_for(CheckScope::Frontend, &CheckPlanMode::ExplicitFull);
    assert_eq!(frontend.len(), 2);
    assert_eq!(frontend[0].id, "full.removed-identity");
    assert!(frontend[0].dependencies.is_empty());
    assert_eq!(frontend[0].repository, CheckTaskRepository::CrossRepository);
    assert_eq!(frontend[0].executor, CheckTaskExecutor::RemovedIdentity);
    assert_eq!(frontend[1].id, "full.frontend");
    assert_eq!(frontend[1].dependencies, ["full.removed-identity"]);
    assert_eq!(frontend[1].repository, CheckTaskRepository::Frontend);
    assert_eq!(
        frontend[1].working_directory,
        CheckTaskWorkingDirectory::Frontend
    );
    assert_eq!(frontend[1].executor, CheckTaskExecutor::FullFrontend);
}

#[test]
fn smart_plan_connects_snapshot_producer_contract_and_frontend_once() {
    let graph = WorkspaceGraph {
        package_by_dir: BTreeMap::from([("crates/api".into(), "ryframe-api".into())]),
        reverse_dependencies: BTreeMap::new(),
    };
    let mode = select_check_mode(
        CheckScope::All,
        false,
        &["crates/api/src/routes.rs".into()],
        &["src/views/post.vue".into()],
        &graph,
    );
    assert_eq!(
        task_ids(&mode, CheckScope::All),
        [
            "smart.snapshots-prepare",
            "smart.backend-packages",
            "smart.snapshots-verify",
            "smart.consumer-contract",
            "smart.frontend",
        ]
    );
    let tasks = tasks_for(CheckScope::All, &mode);
    assert_eq!(
        tasks.last().unwrap().dependencies,
        ["smart.consumer-contract"]
    );
}

#[test]
fn smart_frontend_only_and_documentation_plans_do_not_invent_backend_work() {
    let frontend_mode = select_check_mode(
        CheckScope::Frontend,
        false,
        &["Cargo.lock".into()],
        &["src/views/post.vue".into()],
        &WorkspaceGraph::default(),
    );
    assert_eq!(
        task_ids(&frontend_mode, CheckScope::Frontend),
        ["smart.removed-identity", "smart.frontend"]
    );
    let CheckPlanMode::Selected(selection) = frontend_mode else {
        panic!("前端代码变更应生成智能计划");
    };
    assert_eq!(
        selection.frontend_profiles,
        [FrontendProfile::Code].into_iter().collect()
    );

    let docs = select_check_mode(
        CheckScope::All,
        false,
        &["docs/development.md".into()],
        &["README.md".into()],
        &WorkspaceGraph::default(),
    );
    assert!(tasks_for(CheckScope::All, &docs).is_empty());
}

#[test]
fn expanded_full_uses_the_same_nodes_as_explicit_full() {
    let expanded = CheckPlanMode::ExpandedFull("共享工具变更".into());
    assert_eq!(
        task_ids(&expanded, CheckScope::All),
        task_ids(&CheckPlanMode::ExplicitFull, CheckScope::All)
    );
}
