use std::{fs, path::Path};

use super::{
    check::TaskExecutor,
    ci::{ci_execution_plan_for, security_report_command, security_source_command},
    cli::{CiCommand, SecurityCommand, SecurityReportKind, SecurityReportOptions},
};

#[test]
fn security_source_uses_one_ordered_task_plan_and_fixed_commands() {
    let plan = ci_execution_plan_for(&CiCommand::Security(SecurityCommand::Source)).unwrap();
    let expected = [
        (
            TaskExecutor::PythonEnvironment,
            "python.environment",
            "python",
            &["tools/python/check_python_environment.py"][..],
        ),
        (
            TaskExecutor::CiSupplyChainSource,
            "ci.security.supply-chain",
            "python",
            &["tools/python/check_supply_chain.py", "--verify-cargo-graph"][..],
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
fn cyclonedx_report_has_one_read_only_rust_node_without_python_or_source_tasks() {
    let options = SecurityReportOptions {
        kind: SecurityReportKind::CycloneDx,
        input: std::env::current_dir()
            .unwrap()
            .join("target/供应链 report.json"),
        require_reproducible: true,
    };
    let command = CiCommand::Security(SecurityCommand::Report(options.clone()));
    let plan = ci_execution_plan_for(&command).unwrap();
    assert_eq!(plan.tasks.len(), 1);
    let task = &plan.tasks[0];
    assert_eq!(task.executor, TaskExecutor::CiCycloneDxReport);
    assert!(task.dependencies.is_empty());
    assert!(task.definition().allowed_writes.is_empty());
    assert!(task.definition().external_resources.is_empty());
    assert!(task.definition().compilation_coverage.is_empty());
    assert!(security_report_command(task.executor, &options).is_err());
    assert!(security_report_command(TaskExecutor::PythonEnvironment, &options).is_err());
}

#[test]
fn trivy_report_preserves_its_typed_node_and_python_prerequisite() {
    let input = std::env::current_dir()
        .unwrap()
        .join("target/供应链 report.json");
    let options = SecurityReportOptions {
        kind: SecurityReportKind::Trivy,
        input: input.clone(),
        require_reproducible: false,
    };
    let command = CiCommand::Security(SecurityCommand::Report(options.clone()));
    let plan = ci_execution_plan_for(&command).unwrap();
    assert_eq!(
        plan.tasks
            .iter()
            .map(|task| task.executor)
            .collect::<Vec<_>>(),
        [TaskExecutor::PythonEnvironment, TaskExecutor::CiTrivyReport]
    );
    assert_eq!(plan.tasks[1].dependencies, vec!["python.environment"]);
    assert_eq!(
        security_report_command(TaskExecutor::PythonEnvironment, &options).unwrap(),
        (
            "python",
            vec!["tools/python/check_python_environment.py".to_owned()]
        )
    );
    assert_eq!(
        security_report_command(TaskExecutor::CiTrivyReport, &options).unwrap(),
        (
            "python",
            vec![
                "tools/python/check_supply_chain.py".to_owned(),
                "--trivy-report".to_owned(),
                input.to_str().unwrap().to_owned()
            ]
        )
    );
    assert!(security_report_command(TaskExecutor::CiCycloneDxReport, &options).is_err());
}

#[test]
fn security_report_planning_rejects_options_that_bypass_the_cli() {
    let relative = SecurityReportOptions {
        kind: SecurityReportKind::CycloneDx,
        input: "relative.json".into(),
        require_reproducible: false,
    };
    assert!(
        ci_execution_plan_for(&CiCommand::Security(SecurityCommand::Report(relative))).is_err()
    );
    let invalid_trivy = SecurityReportOptions {
        kind: SecurityReportKind::Trivy,
        input: std::env::current_dir().unwrap().join("trivy.json"),
        require_reproducible: true,
    };
    assert!(
        ci_execution_plan_for(&CiCommand::Security(SecurityCommand::Report(invalid_trivy)))
            .is_err()
    );
}

#[test]
fn extended_workflow_generates_uploads_and_uses_only_typed_report_checks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let workflow = fs::read_to_string(root.join(".github/workflows/extended-ci.yml")).unwrap();
    assert_eq!(
        workflow
            .matches("cargo xtask check ci security report cyclonedx")
            .count(),
        2
    );
    assert_eq!(
        workflow
            .matches("cargo xtask check ci security report trivy")
            .count(),
        1
    );
    assert!(!workflow.contains("python tools/python/check_supply_chain.py"));
    assert!(workflow.contains("cargo cyclonedx \\"));
    assert_eq!(workflow.matches("trivy image \\").count(), 2);
    assert!(workflow.contains("actions/upload-artifact@"));
    assert!(workflow.contains("if-no-files-found: error"));
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
    assert!(!security.contains("python tools/python/check_supply_chain.py"));
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
    assert!(!security.contains("python tools/python/check_deployment_assets.py"));
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
