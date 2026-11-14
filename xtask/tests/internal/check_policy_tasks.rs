use std::collections::BTreeSet;

use super::check::{POLICY_TASKS, PolicyProfile, PolicyTaskInvocation, policy_tasks};

#[test]
fn policy_tasks_have_unique_ids() {
    let ids = POLICY_TASKS
        .iter()
        .map(|task| task.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), POLICY_TASKS.len());
}

#[test]
fn full_and_ci_preflight_run_the_full_stack_workflow_policy() {
    let definitions = |profile| {
        policy_tasks(profile)
            .into_iter()
            .map(|task| (task.id, task.invocation))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        definitions(PolicyProfile::FullStatic),
        [
            ("supply-chain", PolicyTaskInvocation::SupplyChain),
            (
                "full-stack-workflow",
                PolicyTaskInvocation::FullStackWorkflow
            ),
        ]
    );
    assert_eq!(
        definitions(PolicyProfile::Smart),
        [("migration-history", PolicyTaskInvocation::MigrationHistory)]
    );
    assert_eq!(
        definitions(PolicyProfile::CiPreflight),
        [(
            "full-stack-workflow",
            PolicyTaskInvocation::FullStackWorkflow
        )]
    );
}
