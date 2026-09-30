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
fn security_reports_use_one_typed_node_after_the_python_prerequisite() {
    let input = std::env::current_dir()
        .unwrap()
        .join("target/供应链 report.json");
    for (kind, report_task, option, reproducible) in [
        (
            SecurityReportKind::CycloneDx,
            TaskExecutor::CiCycloneDxReport,
            "--cyclonedx",
            true,
        ),
        (
            SecurityReportKind::Trivy,
            TaskExecutor::CiTrivyReport,
            "--trivy-report",
            false,
        ),
    ] {
        let options = SecurityReportOptions {
            kind,
            input: input.clone(),
            require_reproducible: reproducible,
        };
        let command = CiCommand::Security(SecurityCommand::Report(options.clone()));
        let plan = ci_execution_plan_for(&command).unwrap();
        assert_eq!(
            plan.tasks
                .iter()
                .map(|task| task.executor)
                .collect::<Vec<_>>(),
            [TaskExecutor::PythonEnvironment, report_task]
        );
        assert_eq!(plan.tasks[1].dependencies, vec!["python.environment"]);
        assert!(plan.tasks.iter().all(|task| !matches!(
            task.executor,
            TaskExecutor::CiSupplyChainSource
                | TaskExecutor::CiCargoAudit
                | TaskExecutor::CiCargoDeny
        )));
        assert_eq!(
            security_report_command(TaskExecutor::PythonEnvironment, &options).unwrap(),
            (
                "python",
                vec!["tools/python/check_python_environment.py".to_owned()]
            )
        );
        let (_, arguments) = security_report_command(report_task, &options).unwrap();
        assert_eq!(
            arguments[..3],
            [
                "tools/python/check_supply_chain.py",
                option,
                input.to_str().unwrap(),
            ]
        );
        assert_eq!(
            arguments.get(3).map(String::as_str),
            reproducible.then_some("--require-reproducible-cyclonedx")
        );
    }
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
