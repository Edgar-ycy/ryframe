//! 智能检查的命令入口与稳定测试接口。

#[path = "check/cargo_command.rs"]
mod cargo_command;
#[path = "check/change_surface.rs"]
mod change_surface;
#[path = "check/context.rs"]
mod context;
#[path = "check/execution.rs"]
mod execution;
#[path = "check/feature.rs"]
mod feature;
#[path = "check/metrics.rs"]
pub(crate) mod metrics;
#[path = "check/model.rs"]
mod model;
#[path = "check/plan.rs"]
mod plan;
#[path = "check/policy_tasks.rs"]
mod policy_tasks;
#[path = "check/resource.rs"]
mod resource;
#[path = "check/selection.rs"]
mod selection;
#[path = "check/snapshot.rs"]
mod snapshot;

pub(crate) use cargo_command::ci_test_jobs_from;
#[allow(unused_imports)]
pub(crate) use cargo_command::{
    WORKSPACE_CLIPPY_ARGS, backend_package_operation_args, cargo_operation_jobs,
    default_test_jobs_from, workspace_clippy_args, workspace_test_args,
};
#[allow(unused_imports)]
pub(crate) use change_surface::{
    ChangeCategory, ChangeSurfacePolicy, ChangeSurfaceReport, RepositoryKind,
    analyze_change_surface, append_changed_file_size_warnings, load_change_surface_policy,
    parse_change_surface_policy,
};
#[allow(unused_imports)]
pub(crate) use context::{
    BACKEND_CI_TARGET_DIR, BACKEND_SMART_TARGET_DIR, BACKEND_VERIFY_TARGET_DIR,
    RESOURCE_CI_TARGET_DIR, RESOURCE_VERIFY_TARGET_DIR, VerifyExecutionContext, VerifyJobBudget,
    VerifyTargetPolicy, ci_environment_from, ci_target_policy, ci_target_policy_from,
    resolve_target_dir, verify_job_budget_from, verify_target_policy_from,
};
#[allow(unused_imports)]
pub(crate) use execution::verify;
#[allow(unused_imports)]
pub(crate) use execution::{
    PYTHON_ENVIRONMENT_ARGS, PYTHON_TEST_ARGS, SMART_BACKEND_OPERATIONS, SMART_FEATURE_OPERATIONS,
};
pub(crate) use execution::{
    ci_consumer_contract, ci_consumer_contract_against_committed_snapshot, ci_rust_gate,
};
#[allow(unused_imports)]
pub(crate) use feature::{
    feature_operation_args, feature_test_args, minimal_workspace_check_args,
    validate_feature_combination,
};
#[allow(unused_imports)]
pub(crate) use model::{
    BackendSnapshotProfile, ConsumerContractPlan, FrontendProfile, VerifySelection, WorkspaceGraph,
};
#[allow(unused_imports)]
pub(crate) use plan::{
    CheckPlanMode, CheckTask, CheckTaskExecutor, CheckTaskRepository, CheckTaskStage,
    CheckTaskWorkingDirectory, TaskPlan, build_task_plan, plan, select_check_mode, tasks_for,
    validate_plan,
};
#[allow(unused_imports)]
pub(crate) use policy_tasks::{PYTHON_POLICY_TASKS, PolicyProfile, PythonPolicyTask, policy_tasks};
#[allow(unused_imports)]
pub(crate) use resource::{
    ResourceWorkspaceProfile, resolve_frontend_dir, resource_test_executable_from_messages,
    resource_workspace_compilation, resource_workspace_environment_for_profile,
    targeted_resource_workspace_compilation,
};
#[allow(unused_imports)]
pub(crate) use selection::{
    changed_paths, changed_paths_between, classify_changes, complete_verify_selection,
    frontend_profile_commands, load_resource_workspace_graph, load_workspace_graph,
    needs_consumer_contract, reverse_dependency_closure,
};
#[allow(unused_imports)]
pub(crate) use snapshot::{
    BackendSnapshots, backend_snapshot_export_args, consumer_contract_arguments,
    consumer_contract_command, consumer_contract_plan, load_consumer_contract_plan,
    package_tests_generate_snapshots, prepare_backend_snapshots,
    prepare_consumer_backend_snapshots, verify_backend_snapshots,
};
