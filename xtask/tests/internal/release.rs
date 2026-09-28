use std::{collections::BTreeMap, fs, path::PathBuf};

use super::{
    check::TaskExecutor,
    cli::{CheckCommand, Command, ReleaseCiOperation, ReleaseCommand, parse},
    release::{PROTOCOL_KEYS, ci_invocation, plan, source_invocation},
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

fn environment(values: &[(&'static str, String)]) -> BTreeMap<&'static str, String> {
    values.iter().cloned().collect()
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
fn source_and_evidence_use_fixed_scripts_and_typed_environment() {
    let frontend = workspace_path("前端 existing 空格");
    fs::create_dir_all(&frontend).unwrap();
    let source_output = workspace_path("证据/source.json");
    let (source, parsed_frontend) = release_command(source_cli(
        source_output.to_str().unwrap(),
        frontend.to_str().unwrap(),
    ));
    let ReleaseCommand::Source(source) = source else {
        panic!("应为来源核验");
    };
    let invocation = source_invocation(&source, &parsed_frontend).unwrap();
    assert_eq!(invocation.script, "scripts/validate_release.py");
    let source_environment = environment(&invocation.environment);
    assert_eq!(source_environment["RYFRAME_RELEASE_PROTOCOL_VERSION"], "1");
    assert_eq!(source_environment["RYFRAME_RELEASE_MODE"], "source");
    assert_eq!(
        source_environment["RYFRAME_RELEASE_FRONTEND_DIR"],
        frontend.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(
        source_environment["RYFRAME_RELEASE_MANIFEST_PATH"],
        source_output.to_str().unwrap()
    );
    assert!(!source_environment.contains_key("GH_TOKEN"));
    assert!(!PROTOCOL_KEYS.contains(&"GH_TOKEN"));
    assert!(!PROTOCOL_KEYS.contains(&"GITHUB_RUN_ID"));
    assert!(!PROTOCOL_KEYS.contains(&"GITHUB_RUN_ATTEMPT"));

    let ci_output = workspace_path("证据/ci.json");
    let (ci, _) = release_command(ci_cli(ci_output.to_str().unwrap()));
    let ReleaseCommand::Ci(ci) = ci else {
        panic!("应为 CI 核验");
    };
    let invocation = ci_invocation(&ci, &frontend).unwrap();
    assert_eq!(invocation.script, "scripts/verify_release_ci.py");
    let ci_environment = environment(&invocation.environment);
    assert_eq!(ci_environment["RYFRAME_RELEASE_MODE"], "ci-evidence");
    assert_eq!(
        ci_environment["RYFRAME_RELEASE_BACKEND_SHA"],
        "a".repeat(40)
    );
    assert_eq!(
        ci_environment["RYFRAME_RELEASE_FRONTEND_SHA"],
        "b".repeat(40)
    );
    assert_eq!(
        ci_environment["RYFRAME_RELEASE_OUTPUT_PATH"],
        ci_output.to_str().unwrap()
    );
}

#[test]
fn source_pair_modes_bind_current_worktrees_through_protocol() {
    let frontend = workspace_path("frontend pair");
    fs::create_dir_all(&frontend).unwrap();
    for (operation, mode, path_option, protocol_path) in [
        (
            "record-pair",
            "ci-record-pair",
            "--output",
            "RYFRAME_RELEASE_OUTPUT_PATH",
        ),
        (
            "verify-pair",
            "ci-verify-pair",
            "--input",
            "RYFRAME_RELEASE_INPUT_PATH",
        ),
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
        let ReleaseCommand::Ci(options) = command else {
            panic!("应为 CI 核验");
        };
        assert!(matches!(
            (&options.operation, operation),
            (ReleaseCiOperation::RecordPair { .. }, "record-pair")
                | (ReleaseCiOperation::VerifyPair { .. }, "verify-pair")
        ));
        let invocation = ci_invocation(&options, &parsed_frontend).unwrap();
        assert_eq!(invocation.script, "scripts/verify_release_ci.py");
        let values = environment(&invocation.environment);
        assert_eq!(values["RYFRAME_RELEASE_MODE"], mode);
        assert_eq!(values[protocol_path], receipt.to_str().unwrap());
        assert_eq!(
            values["RYFRAME_RELEASE_FRONTEND_DIR"],
            frontend.canonicalize().unwrap().to_str().unwrap()
        );
        assert!(values.contains_key("RYFRAME_RELEASE_BACKEND_DIR"));
    }
}

#[test]
fn protocol_rejects_paths_with_line_breaks_before_execution() {
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
