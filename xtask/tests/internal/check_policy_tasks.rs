use std::collections::BTreeSet;

use super::check::{
    MIGRATION_HISTORY_SCRIPT, PYTHON_POLICY_TASKS, PolicyProfile, STRICT_MIGRATION_HISTORY_ARGS,
    policy_tasks,
};

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
        scripts(PolicyProfile::FullStatic),
        [
            "tools/python/check_architecture.py",
            "tools/python/check_deployment_assets.py",
            "tools/python/check_permission_routes.py",
            "tools/python/check_supply_chain.py",
        ]
    );
    assert_eq!(
        scripts(PolicyProfile::Smart),
        [
            "tools/python/check_architecture.py",
            "tools/python/check_migration_history.py",
            "tools/python/check_permission_routes.py",
        ]
    );
    assert_eq!(
        scripts(PolicyProfile::CiPreflight),
        [
            "tools/python/check_architecture.py",
            "tools/python/check_permission_routes.py",
        ]
    );
    assert_eq!(
        scripts(PolicyProfile::FullStatic)
            .iter()
            .filter(|script| **script == "tools/python/check_supply_chain.py")
            .count(),
        1
    );
    assert!(
        !scripts(PolicyProfile::CiPreflight).contains(&"tools/python/check_supply_chain.py"),
        "CI preflight 的供应链检查由始终执行的 security source job 覆盖"
    );
    assert!(
        !scripts(PolicyProfile::CiPreflight).contains(&"tools/python/check_deployment_assets.py"),
        "CI preflight 的部署静态检查由始终执行的 security deployment source 覆盖"
    );
}

#[test]
fn strict_migration_command_reuses_the_registered_script() {
    let registered = PYTHON_POLICY_TASKS
        .iter()
        .find(|task| task.id == "migration-history")
        .unwrap();
    assert_eq!(registered.script, MIGRATION_HISTORY_SCRIPT);
    assert_eq!(
        STRICT_MIGRATION_HISTORY_ARGS,
        [MIGRATION_HISTORY_SCRIPT, "--require-frozen"]
    );
    assert!(
        policy_tasks(PolicyProfile::FullStatic)
            .into_iter()
            .all(|task| task.id != "migration-history")
    );
}

#[test]
fn python_policies_only_use_their_registered_script() {
    for task in PYTHON_POLICY_TASKS {
        assert_ne!(task.id, "removed-identity");
        assert_ne!(task.id, "prerelease-dependencies");
        assert_eq!(task.arguments(), [task.script]);
    }
}
