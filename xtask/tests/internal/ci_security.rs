use std::{fs, path::Path};

use super::{
    check::TaskExecutor,
    ci::{ci_execution_plan_for, security_source_command},
    cli::{CiCommand, SecurityCommand},
};

#[test]
fn security_source_uses_one_ordered_task_plan_and_fixed_commands() {
    let plan = ci_execution_plan_for(&CiCommand::Security(SecurityCommand::Source)).unwrap();
    let expected = [
        (
            TaskExecutor::PythonEnvironment,
            "python.environment",
            "python",
            &["scripts/check_python_environment.py"][..],
        ),
        (
            TaskExecutor::CiSupplyChainSource,
            "ci.security.supply-chain",
            "python",
            &["scripts/check_supply_chain.py", "--verify-cargo-graph"][..],
        ),
        (
            TaskExecutor::CiCargoAudit,
            "ci.security.audit",
            "cargo",
            &["audit", "--deny", "warnings"][..],
        ),
        (
            TaskExecutor::CiCargoDeny,
            "ci.security.deny",
            "cargo",
            &["deny", "check", "licenses", "bans", "sources"][..],
        ),
    ];

    assert_eq!(plan.tasks.len(), expected.len());
    for (index, (task, (executor, id, program, arguments))) in
        plan.tasks.iter().zip(expected).enumerate()
    {
        assert_eq!(task.executor, executor);
        assert_eq!(task.id, id);
        assert_eq!(
            security_source_command(executor).unwrap(),
            (program, arguments)
        );
        let expected_dependencies = if index == 0 {
            Vec::new()
        } else {
            vec![plan.tasks[index - 1].id]
        };
        assert_eq!(task.dependencies, expected_dependencies);
    }
}

#[test]
fn security_workflow_installs_tools_then_uses_only_typed_security_entries() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let workflow = fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    let security = workflow
        .split("\n  security-audit:\n")
        .nth(1)
        .unwrap()
        .split("\n  #")
        .next()
        .unwrap();

    assert_eq!(
        security
            .matches("cargo xtask check ci security source")
            .count(),
        1
    );
    assert!(!security.contains("python scripts/check_supply_chain.py"));
    assert!(!security.contains("cargo audit --deny warnings"));
    assert!(!security.contains("cargo deny check licenses bans sources"));
    assert_eq!(
        security
            .matches("cargo xtask check ci security deployment source")
            .count(),
        1
    );
    assert_eq!(
        security
            .matches("cargo xtask check ci security deployment image")
            .count(),
        1
    );
    assert!(!security.contains("python scripts/check_deployment_assets.py"));
    assert!(!security.contains("git diff --quiet"));
    assert!(!security.contains("docker compose"));
    assert!(!security.contains("docker run"));
    assert_eq!(security.matches("docker build").count(), 1);
    assert_eq!(
        security
            .matches("steps.deployment.outputs.required == 'true'")
            .count(),
        2
    );
    assert!(security.contains("tool: cargo-audit@0.22.2"));
    assert!(security.contains("tool: cargo-deny@0.20.2"));
    let source = security
        .find("cargo xtask check ci security source")
        .unwrap();
    assert!(security.find("tool: cargo-audit@0.22.2").unwrap() < source);
    assert!(security.find("tool: cargo-deny@0.20.2").unwrap() < source);
    let deployment_source = security
        .find("cargo xtask check ci security deployment source")
        .unwrap();
    let build = security.find("docker build").unwrap();
    let deployment_image = security
        .find("cargo xtask check ci security deployment image")
        .unwrap();
    assert!(source < deployment_source);
    assert!(deployment_source < build);
    assert!(build < deployment_image);
    assert!(!security.contains("github.event.action != 'edited'"));
}
