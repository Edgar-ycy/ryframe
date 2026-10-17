use std::collections::BTreeSet;

use super::check::{
    POLICY_TASKS, PolicyProfile, PolicyTaskInvocation, STRICT_MIGRATION_HISTORY_ARGS, policy_tasks,
};

#[test]
fn policy_tasks_have_unique_stable_ids_and_python_scripts() {
    let ids = POLICY_TASKS
        .iter()
        .map(|task| task.id)
        .collect::<BTreeSet<_>>();
    let scripts = POLICY_TASKS
        .iter()
        .filter_map(|task| task.python_script())
        .collect::<BTreeSet<_>>();

    assert_eq!(ids.len(), POLICY_TASKS.len());
    assert_eq!(scripts.len() + 1, POLICY_TASKS.len());
    assert!(
        POLICY_TASKS
            .iter()
            .any(|task| task.invocation == PolicyTaskInvocation::MigrationHistory)
    );
}

#[test]
fn smart_and_ci_profiles_reuse_the_same_policy_task_definitions() {
    let definitions = |profile| {
        policy_tasks(profile)
            .into_iter()
            .map(|task| (task.id, task.python_script()))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        definitions(PolicyProfile::FullStatic),
        [("supply-chain", Some("tools/python/check_supply_chain.py"))]
    );
    assert_eq!(
        definitions(PolicyProfile::Smart),
        [("migration-history", None)]
    );
    assert_eq!(definitions(PolicyProfile::CiPreflight), []);
    assert_eq!(
        definitions(PolicyProfile::FullStatic)
            .iter()
            .filter(|(_id, script)| *script == Some("tools/python/check_supply_chain.py"))
            .count(),
        1
    );
    assert!(
        !definitions(PolicyProfile::CiPreflight)
            .iter()
            .any(|(_id, script)| *script == Some("tools/python/check_supply_chain.py")),
        "CI preflight 的供应链检查由始终执行的 security source job 覆盖"
    );
    for retired in [
        "tools/python/check_architecture.py",
        "tools/python/check_deployment_assets.py",
        "tools/python/check_permission_routes.py",
        "tools/python/check_migration_history.py",
    ] {
        assert!(
            POLICY_TASKS
                .iter()
                .filter_map(|task| task.python_script())
                .all(|script| script != retired)
        );
    }
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

#[test]
fn python_policies_only_use_their_registered_script() {
    for task in POLICY_TASKS {
        assert_ne!(task.id, "removed-identity");
        assert_ne!(task.id, "prerelease-dependencies");
        if let Some(script) = task.python_script() {
            assert!(script.starts_with("tools/python/"));
        }
    }
}
