use std::collections::BTreeSet;

use super::check::{
    POLICY_TASKS, PolicyProfile, PolicyTaskInvocation, STRICT_MIGRATION_HISTORY_ARGS, policy_tasks,
};

#[test]
fn policy_tasks_have_unique_stable_ids_and_native_invocations() {
    let ids = POLICY_TASKS
        .iter()
        .map(|task| task.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), POLICY_TASKS.len());
    assert!(
        POLICY_TASKS
            .iter()
            .any(|task| task.invocation == PolicyTaskInvocation::MigrationHistory)
    );
    assert!(
        POLICY_TASKS
            .iter()
            .any(|task| task.invocation == PolicyTaskInvocation::SupplyChain)
    );
}

#[test]
fn smart_and_ci_profiles_reuse_the_same_policy_task_definitions() {
    let definitions = |profile| {
        policy_tasks(profile)
            .into_iter()
            .map(|task| (task.id, task.invocation))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        definitions(PolicyProfile::FullStatic),
        [("supply-chain", PolicyTaskInvocation::SupplyChain)]
    );
    assert_eq!(
        definitions(PolicyProfile::Smart),
        [("migration-history", PolicyTaskInvocation::MigrationHistory)]
    );
    assert_eq!(definitions(PolicyProfile::CiPreflight), []);
}

#[test]
fn strict_migration_command_uses_the_native_registered_task() {
    let registered = POLICY_TASKS
        .iter()
        .find(|task| task.id == "migration-history")
        .unwrap();
    assert_eq!(
        registered.invocation,
        PolicyTaskInvocation::MigrationHistory
    );
    assert_eq!(STRICT_MIGRATION_HISTORY_ARGS, ["--require-frozen"]);
    assert!(
        policy_tasks(PolicyProfile::FullStatic)
            .into_iter()
            .all(|task| task.id != "migration-history")
    );
}
