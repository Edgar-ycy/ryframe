use std::collections::BTreeSet;
use std::path::Path;

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
            "scripts/check_architecture.py",
            "scripts/check_deployment_assets.py",
            "scripts/check_prerelease_dependencies.py",
            "scripts/check_permission_routes.py",
            "scripts/check_removed_identity.py",
            "scripts/check_supply_chain.py",
        ]
    );
    assert_eq!(
        scripts(PolicyProfile::Smart),
        [
            "scripts/check_architecture.py",
            "scripts/check_migration_history.py",
            "scripts/check_permission_routes.py",
            "scripts/check_removed_identity.py",
        ]
    );
    assert_eq!(
        scripts(PolicyProfile::CiPreflight),
        [
            "scripts/check_prerelease_dependencies.py",
            "scripts/check_supply_chain.py",
            "scripts/check_architecture.py",
            "scripts/check_permission_routes.py",
            "scripts/check_removed_identity.py",
            "scripts/check_deployment_assets.py",
        ]
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
fn removed_identity_task_binds_the_frontend_directory_without_splitting_it() {
    let root = std::env::current_dir().unwrap();
    let frontend = Path::new("../包含 空格/ryframe-vue3");
    let task = PYTHON_POLICY_TASKS
        .iter()
        .find(|task| task.id == "removed-identity")
        .unwrap();

    let arguments = task.arguments(&root, frontend).unwrap();
    assert_eq!(arguments[0], "scripts/check_removed_identity.py");
    assert_eq!(arguments[1], "--frontend-dir");
    assert!(Path::new(&arguments[2]).is_absolute());
    assert_eq!(
        Path::new(&arguments[2]),
        std::path::absolute(root.join(frontend)).unwrap()
    );
    assert_eq!(
        PYTHON_POLICY_TASKS[0].arguments(&root, frontend).unwrap(),
        ["scripts/check_architecture.py"]
    );
}
