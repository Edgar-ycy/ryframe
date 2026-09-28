use std::collections::{BTreeMap, BTreeSet};

use super::{
    check::{
        CheckPlanMode, FrontendProfile, TASK_REGISTRY, TaskExecutor, TaskRepository, TaskStage,
        TaskWorkingDirectory, WorkspaceGraph, select_check_mode, tasks_for,
    },
    cli::CheckScope,
};

fn task_ids(mode: &CheckPlanMode, scope: CheckScope) -> Vec<&'static str> {
    tasks_for(scope, mode)
        .unwrap()
        .tasks
        .into_iter()
        .map(|task| task.id)
        .collect()
}

#[test]
fn full_all_plan_exposes_the_executed_topology_and_metadata() {
    let plan = tasks_for(CheckScope::All, &CheckPlanMode::ExplicitFull).unwrap();
    assert_eq!(
        plan.tasks.iter().map(|task| task.id).collect::<Vec<_>>(),
        [
            "python.environment",
            "frontend.dependencies",
            "rust.feature-registry",
            "rust.format",
            "rust.workspace-clippy",
            "policy.complete",
            "contract.snapshots-prepare",
            "python.tests",
            "node.tests",
            "migration.history",
            "rust.workspace-gates",
            "contract.snapshots-verify",
            "contract.consumer-full",
        ]
    );
    let mut seen = BTreeSet::new();
    for task in &plan.tasks {
        assert!(
            task.dependencies
                .iter()
                .all(|dependency| seen.contains(dependency))
        );
        assert!(!task.executor.label().is_empty());
        assert!(!task.executor.description().is_empty());
        assert!(task.definition().external_resources.is_empty());
        assert_eq!(task.definition().id, task.id);
        seen.insert(task.id);
    }
    let workspaces = plan
        .tasks
        .iter()
        .find(|task| task.executor == TaskExecutor::WorkspaceGates)
        .unwrap();
    let definition = workspaces.definition();
    assert_eq!(definition.repository, TaskRepository::CrossRepository);
    assert_eq!(definition.stage, TaskStage::Test);
    assert_eq!(definition.working_directory, TaskWorkingDirectory::Backend);
    assert!(definition.compilation_coverage.contains(&"feature matrix"));
    assert!(definition.allowed_writes.contains(&"并行任务日志"));

    let migrations = plan
        .tasks
        .iter()
        .filter(|task| task.executor == TaskExecutor::MigrationHistory)
        .collect::<Vec<_>>();
    assert_eq!(migrations.len(), 1, "完整任务图只能执行一次迁移历史检查");
    assert_eq!(migrations[0].id, "migration.history");
    assert_eq!(migrations[0].dependencies, ["node.tests"]);
    assert_eq!(
        migrations[0].executor.static_arguments(),
        Some(["scripts/check_migration_history.py", "--require-frozen"].as_slice())
    );

    let python = &plan.tasks[0];
    assert_eq!(python.definition().repository, TaskRepository::Backend);
    assert_eq!(python.definition().stage, TaskStage::Prerequisite);
    assert_eq!(python.executor, TaskExecutor::PythonEnvironment);
    assert!(python.dependencies.is_empty());
}

#[test]
fn full_scope_keeps_backend_contract_and_frontend_only_paths_distinct() {
    let backend = tasks_for(CheckScope::Backend, &CheckPlanMode::ExplicitFull).unwrap();
    assert_eq!(backend.tasks.last().unwrap().id, "contract.consumer");
    assert_eq!(
        backend.tasks.last().unwrap().executor,
        TaskExecutor::ConsumerContract
    );

    let frontend = tasks_for(CheckScope::Frontend, &CheckPlanMode::ExplicitFull).unwrap();
    assert_eq!(frontend.tasks.len(), 2);
    assert_eq!(frontend.tasks[0].id, "policy.removed-identity");
    assert!(frontend.tasks[0].dependencies.is_empty());
    assert_eq!(
        frontend.tasks[0].definition().repository,
        TaskRepository::CrossRepository
    );
    assert_eq!(frontend.tasks[0].executor, TaskExecutor::RemovedIdentity);
    assert_eq!(frontend.tasks[1].id, "frontend.full");
    assert_eq!(frontend.tasks[1].dependencies, ["policy.removed-identity"]);
    assert_eq!(
        frontend.tasks[1].definition().repository,
        TaskRepository::Frontend
    );
    assert_eq!(
        frontend.tasks[1].definition().working_directory,
        TaskWorkingDirectory::Frontend
    );
    assert_eq!(frontend.tasks[1].executor, TaskExecutor::FrontendFull);
}

#[test]
fn smart_plan_connects_shared_primitives_snapshot_contract_and_frontend_once() {
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
            "contract.snapshots-prepare",
            "rust.feature-registry",
            "rust.format",
            "smart.backend-packages",
            "policy.smart",
            "contract.snapshots-verify",
            "contract.consumer",
            "smart.frontend",
        ]
    );
    let tasks = tasks_for(CheckScope::All, &mode).unwrap();
    assert_eq!(
        tasks.tasks.last().unwrap().dependencies,
        ["contract.consumer"]
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
        ["policy.removed-identity", "smart.frontend"]
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
    assert!(tasks_for(CheckScope::All, &docs).unwrap().tasks.is_empty());
}

#[test]
fn expanded_full_uses_the_same_nodes_as_explicit_full() {
    let expanded = CheckPlanMode::ExpandedFull("共享工具变更".into());
    assert_eq!(
        task_ids(&expanded, CheckScope::All),
        task_ids(&CheckPlanMode::ExplicitFull, CheckScope::All)
    );
}

#[test]
fn primitive_task_registry_is_unique_and_is_the_only_metadata_source() {
    let ids = TASK_REGISTRY
        .iter()
        .map(|definition| definition.id)
        .collect::<BTreeSet<_>>();
    let executors = TASK_REGISTRY
        .iter()
        .map(|definition| definition.executor)
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), TASK_REGISTRY.len());
    assert_eq!(
        executors.iter().collect::<BTreeSet<_>>().len(),
        TASK_REGISTRY.len()
    );
    for definition in TASK_REGISTRY {
        assert_eq!(definition.executor.definition(), definition);
    }
}
