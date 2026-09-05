use std::collections::BTreeSet;

use super::check::{PYTHON_POLICY_TASKS, PolicyProfile, policy_tasks};

#[test]
fn policy_tasks_have_unique_stable_ids_and_script_paths() {
    let ids = PYTHON_POLICY_TASKS
        .iter()
        .map(|task| task.id)
        .collect::<BTreeSet<_>>();
    let scripts = PYTHON_POLICY_TASKS
        .iter()
        .map(|task| task.script)
        .collect::<BTreeSet<_>>();

    assert_eq!(ids.len(), PYTHON_POLICY_TASKS.len());
    assert_eq!(scripts.len(), PYTHON_POLICY_TASKS.len());
}

#[test]
fn smart_and_ci_profiles_reuse_the_same_policy_task_definitions() {
    let scripts = |profile| {
        policy_tasks(profile)
            .into_iter()
            .map(|task| task.script)
            .collect::<Vec<_>>()
    };

    assert_eq!(
        scripts(PolicyProfile::Smart),
        [
            "scripts/check_architecture.py",
            "scripts/check_migration_history.py",
            "scripts/check_permission_routes.py",
        ]
    );
    assert_eq!(
        scripts(PolicyProfile::CiPreflight),
        [
            "scripts/check_prerelease_dependencies.py",
            "scripts/check_supply_chain.py",
            "scripts/check_architecture.py",
            "scripts/check_permission_routes.py",
            "scripts/check_deployment_assets.py",
        ]
    );
}
