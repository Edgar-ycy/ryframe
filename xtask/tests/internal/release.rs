use std::{fs, path::PathBuf};

use super::{
    check::TaskExecutor,
    cli::{CheckCommand, Command, ReleaseCiOperation, ReleaseCommand, parse},
    release::{ci_arguments, plan, source_arguments},
};

fn workspace_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/release-route-tests")
        .join(name)
}

fn release_command(arguments: Vec<String>) -> (ReleaseCommand, PathBuf) {
    let cli = parse(arguments).unwrap();
    let Command::Check(CheckCommand::Release(command)) = cli.command else {
        panic!("应解析为发布核验命令");
    };
    (command, cli.frontend_dir)
}

fn source_cli(output: &str, frontend: &str) -> Vec<String> {
    vec![
        "check".to_owned(),
        "release".to_owned(),
        "source".to_owned(),
        "--tag".to_owned(),
        "v0.12.1".to_owned(),
        "--backend-repository".to_owned(),
        "owner/backend".to_owned(),
        "--backend-commit".to_owned(),
        "a".repeat(40),
        "--frontend-repository".to_owned(),
        "owner/frontend".to_owned(),
        "--frontend-commit".to_owned(),
        "b".repeat(40),
        "--manifest-path".to_owned(),
        output.to_owned(),
        "--frontend-dir".to_owned(),
        frontend.to_owned(),
    ]
}

fn ci_cli(output: &str) -> Vec<String> {
    vec![
        "check".to_owned(),
        "release".to_owned(),
        "ci".to_owned(),
        "--backend-repository".to_owned(),
        "owner/backend".to_owned(),
        "--frontend-repository".to_owned(),
        "owner/frontend".to_owned(),
        "--backend-sha".to_owned(),
        "a".repeat(40),
        "--frontend-sha".to_owned(),
        "b".repeat(40),
        "--backend-tag-oid".to_owned(),
        "c".repeat(40),
        "--frontend-tag-oid".to_owned(),
        "d".repeat(40),
        "--tag".to_owned(),
        "v0.12.1".to_owned(),
        "--timeout".to_owned(),
        "5400".to_owned(),
        "--output".to_owned(),
        output.to_owned(),
    ]
}

#[test]
fn source_and_ci_have_one_registered_task_after_python_environment() {
    let frontend = workspace_path("frontend");
    let output = workspace_path("evidence/source.json");
    let source = release_command(source_cli(
        output.to_str().unwrap(),
        frontend.to_str().unwrap(),
    ))
    .0;
    let ci = release_command(ci_cli(output.to_str().unwrap())).0;
    assert_eq!(
        plan(&source)
            .unwrap()
            .tasks
            .iter()
            .map(|task| task.executor)
            .collect::<Vec<_>>(),
        [TaskExecutor::PythonEnvironment, TaskExecutor::ReleaseSource]
    );
    assert_eq!(
        plan(&ci)
            .unwrap()
            .tasks
            .iter()
            .map(|task| task.executor)
            .collect::<Vec<_>>(),
        [TaskExecutor::PythonEnvironment, TaskExecutor::ReleaseCi]
    );
}

#[test]
fn source_and_ci_commands_preserve_typed_values_once() {
    let frontend = workspace_path("frontend existing");
    fs::create_dir_all(&frontend).unwrap();
    let source_output = workspace_path("evidence/source.json");
    let (source, parsed_frontend) = release_command(source_cli(
        source_output.to_str().unwrap(),
        frontend.to_str().unwrap(),
    ));
    let ReleaseCommand::Source(source) = source else {
        panic!("应为来源核验");
    };
    let arguments = source_arguments(&source, &parsed_frontend).unwrap();
    assert_eq!(arguments[0], "scripts/validate_release.py");
    assert_eq!(
        arguments.iter().filter(|value| *value == "--tag").count(),
        1
    );
    assert!(arguments.contains(&source_output.to_string_lossy().into_owned()));

    let ci_output = workspace_path("evidence/ci.json");
    let (ci, _) = release_command(ci_cli(ci_output.to_str().unwrap()));
    let ReleaseCommand::Ci(ci) = ci else {
        panic!("应为 CI 核验");
    };
    let arguments = ci_arguments(&ci, &frontend).unwrap();
    assert_eq!(arguments[0], "scripts/verify_release_ci.py");
    assert_eq!(
        arguments,
        vec![
            "scripts/verify_release_ci.py".to_owned(),
            "--backend-repository".to_owned(),
            "owner/backend".to_owned(),
            "--frontend-repository".to_owned(),
            "owner/frontend".to_owned(),
            "--backend-sha".to_owned(),
            "a".repeat(40),
            "--frontend-sha".to_owned(),
            "b".repeat(40),
            "--backend-tag-oid".to_owned(),
            "c".repeat(40),
            "--frontend-tag-oid".to_owned(),
            "d".repeat(40),
            "--tag".to_owned(),
            "v0.12.1".to_owned(),
            "--timeout".to_owned(),
            "5400".to_owned(),
            "--output".to_owned(),
            ci_output.to_string_lossy().into_owned(),
        ]
    );
}

#[test]
fn source_pair_modes_bind_current_worktrees() {
    let frontend = workspace_path("frontend pair");
    fs::create_dir_all(&frontend).unwrap();
    for (operation, option) in [
        ("record-pair", "--record-pair"),
        ("verify-pair", "--verify-pair"),
    ] {
        let receipt = workspace_path(&format!("pair/{operation}.json"));
        let path_option = if operation == "record-pair" {
            "--output"
        } else {
            "--input"
        };
        let (command, parsed_frontend) = release_command(vec![
            "check".to_owned(),
            "release".to_owned(),
            "ci".to_owned(),
            operation.to_owned(),
            path_option.to_owned(),
            receipt.to_string_lossy().into_owned(),
            "--frontend-dir".to_owned(),
            frontend.to_string_lossy().into_owned(),
        ]);
        let ReleaseCommand::Ci(options) = command else {
            panic!("应为 CI 核验");
        };
        assert!(matches!(
            (&options.operation, operation),
            (ReleaseCiOperation::RecordPair { .. }, "record-pair")
                | (ReleaseCiOperation::VerifyPair { .. }, "verify-pair")
        ));
        let arguments = ci_arguments(&options, &parsed_frontend).unwrap();
        assert_eq!(arguments[0], "scripts/verify_release_ci.py");
        assert!(arguments.contains(&option.to_owned()));
        assert!(arguments.contains(&receipt.to_string_lossy().into_owned()));
        assert!(arguments.contains(&"--backend-dir".to_owned()));
        assert!(arguments.contains(&"--frontend-dir".to_owned()));
    }
}
