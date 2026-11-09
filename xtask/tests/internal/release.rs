use std::{fs, path::PathBuf};

use super::{
    check::TaskExecutor,
    cli::{CheckCommand, Command, ReleaseCiOperation, ReleaseCommand, parse},
    release::plan,
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
fn source_and_ci_have_one_rust_release_task() {
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
        [TaskExecutor::ReleaseSource]
    );
    assert_eq!(
        plan(&ci)
            .unwrap()
            .tasks
            .iter()
            .map(|task| task.executor)
            .collect::<Vec<_>>(),
        [TaskExecutor::ReleaseCi]
    );
}

#[test]
fn pair_modes_are_typed_and_never_launch_python() {
    let frontend = workspace_path("frontend pair");
    fs::create_dir_all(&frontend).unwrap();
    for (operation, path_option, variant) in [
        ("record-pair", "--output", "record"),
        ("verify-pair", "--input", "verify"),
    ] {
        let receipt = workspace_path(&format!("pair/{operation}.json"));
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
        assert_eq!(parsed_frontend, frontend);
        let ReleaseCommand::Ci(options) = command else {
            panic!("应为 CI 核验");
        };
        assert!(matches!(
            (&options.operation, variant),
            (ReleaseCiOperation::RecordPair { .. }, "record")
                | (ReleaseCiOperation::VerifyPair { .. }, "verify")
        ));
        assert_eq!(
            plan(&ReleaseCommand::Ci(options))
                .unwrap()
                .tasks
                .iter()
                .map(|task| task.executor)
                .collect::<Vec<_>>(),
            [TaskExecutor::ReleaseCi]
        );
    }
}

#[test]
fn protocol_paths_reject_line_breaks_before_execution() {
    let frontend = workspace_path("frontend newline");
    let output = workspace_path("evidence/bad\nreceipt.json");
    let command = release_command(source_cli(
        output.to_str().unwrap(),
        frontend.to_str().unwrap(),
    ))
    .0;
    let error = plan(&command).unwrap_err().to_string();
    assert!(error.contains("换行符"));
}

#[test]
fn plan_is_read_only_and_does_not_require_frontend_or_python() {
    let output = workspace_path("evidence/missing/source.json");
    let frontend = workspace_path("frontend missing");
    let command = release_command(source_cli(
        output.to_str().unwrap(),
        frontend.to_str().unwrap(),
    ))
    .0;
    assert!(!output.parent().unwrap().exists());
    let planned = plan(&command).unwrap();
    assert_eq!(planned.tasks.len(), 1);
    assert!(!output.parent().unwrap().exists());
}
