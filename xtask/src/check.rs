//! 智能检查的命令入口与稳定测试接口。

#[path = "check/change_surface.rs"]
mod change_surface;
#[path = "check/execution.rs"]
mod execution;
#[path = "check/feature.rs"]
mod feature;
#[path = "check/model.rs"]
mod model;
#[path = "check/selection.rs"]
mod selection;
#[path = "check/snapshot.rs"]
mod snapshot;

#[allow(unused_imports)]
pub(crate) use execution::{run, verify};
#[allow(unused_imports)]
pub(crate) use feature::feature_matrix;

#[allow(unused_imports)]
pub(crate) use change_surface::{
    ChangeCategory, ChangeSurfacePolicy, ChangeSurfaceReport, RepositoryKind,
    analyze_change_surface, load_change_surface_policy,
};
#[allow(unused_imports)]
pub(crate) use execution::{
    BACKEND_POLICY_SCRIPTS, BACKEND_VERIFY_TARGET_DIR, CONSUMER_OWNED_COMMANDS,
    FRONTEND_FULL_NON_CONSUMER_COMMANDS, FRONTEND_ONLY_CONTRACT_COMMANDS, PYTHON_TEST_ARGS,
    RESOURCE_VERIFY_TARGET_DIR, WORKSPACE_CLIPPY_ARGS, WORKSPACE_TEST_ARGS,
};
#[allow(unused_imports)]
pub(crate) use feature::{feature_operation_args, feature_test_args, validate_feature_combination};
#[allow(unused_imports)]
pub(crate) use model::{
    BackendSnapshotProfile, ConsumerContractPlan, FrontendProfile, VerifySelection, WorkspaceGraph,
};
#[allow(unused_imports)]
pub(crate) use selection::{
    changed_paths, classify_changes, complete_verify_selection, frontend_profile_commands,
    load_workspace_graph, needs_consumer_contract, reverse_dependency_closure,
};
#[allow(unused_imports)]
pub(crate) use snapshot::{
    backend_snapshot_export_args, consumer_contract_arguments, consumer_contract_plan,
    load_consumer_contract_plan,
};
