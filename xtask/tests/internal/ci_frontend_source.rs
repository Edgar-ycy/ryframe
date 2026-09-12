use std::path::PathBuf;

use super::{
    check::TaskExecutor,
    ci::ci_execution_plan_for,
    cli::{CheckCommand, CiCommand, Command, FrontendSourceEvent, FrontendSourceOptions, parse},
};

fn command(arguments: &[&str]) -> Result<Command, String> {
    parse(arguments.iter().map(|value| (*value).to_owned()).collect())
        .map(|cli| cli.command)
        .map_err(|error| error.to_string())
}

fn absolute(name: &str) -> PathBuf {
    std::env::current_dir().unwrap().join(name)
}

#[test]
fn parses_complete_frontend_source_inputs() {
    let event = absolute("target/中文 event.json");
    let candidate = absolute("target/候选 openapi.json");
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let actual = command(&[
        "check",
        "ci",
        "frontend-source",
        "--event-name",
        "pull_request",
        "--event",
        event.to_str().unwrap(),
        "--base-sha",
        sha,
        "--prefer-marker",
        "--candidate-openapi",
        candidate.to_str().unwrap(),
        "--release-ref",
        "refs/pull/42/merge",
    ])
    .unwrap();
    assert_eq!(
        actual,
        Command::Check(CheckCommand::Ci(CiCommand::FrontendSource(
            FrontendSourceOptions {
                event_name: FrontendSourceEvent::PullRequest,
                event: Some(event),
                base_sha: Some(sha.to_owned()),
                prefer_marker: true,
                candidate_openapi: Some(candidate),
                release_ref: Some("refs/pull/42/merge".to_owned()),
                fallback_main_on_invalid_base: false,
            }
        )))
    );
}

#[test]
fn preserves_empty_github_base_and_ref_for_non_pull_events() {
    let actual = command(&[
        "check",
        "ci",
        "frontend-source",
        "--event-name",
        "push",
        "--base-sha",
        "",
        "--release-ref",
        "",
    ])
    .unwrap();
    let Command::Check(CheckCommand::Ci(CiCommand::FrontendSource(options))) = actual else {
        panic!("必须解析为前端来源命令");
    };
    assert_eq!(options.event_name, FrontendSourceEvent::Push);
    assert_eq!(options.base_sha, None);
    assert_eq!(options.release_ref, None);
}

#[test]
fn pull_request_empty_base_requires_explicit_fallback() {
    let event = absolute("target/event.json");
    let valid = command(&[
        "check",
        "ci",
        "frontend-source",
        "--event-name",
        "pull_request",
        "--event",
        event.to_str().unwrap(),
        "--base-sha",
        "",
        "--fallback-main-on-invalid-base",
    ]);
    assert!(valid.is_ok());

    let invalid = command(&[
        "check",
        "ci",
        "frontend-source",
        "--event-name",
        "pull_request",
        "--event",
        event.to_str().unwrap(),
        "--base-sha",
        "",
    ]);
    assert!(invalid.is_err());
}

#[test]
fn rejects_ambiguous_or_untrusted_frontend_source_inputs() {
    let event = absolute("target/event.json");
    let newline_event = format!("{}\nother.json", event.display());
    let invalid = [
        vec!["check", "ci", "frontend-source"],
        vec!["check", "ci", "frontend-source", "--event-name", "unknown"],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "pull_request",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--event",
            "relative",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--event",
            newline_event.as_str(),
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--backend-worktree",
            ".",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--base-sha",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--release-ref",
            "refs/heads/main\nsecond",
        ],
    ];
    for arguments in invalid {
        assert!(command(&arguments).is_err(), "参数应被拒绝：{arguments:?}");
    }
}

#[test]
fn rejects_duplicate_or_mutually_exclusive_frontend_source_inputs() {
    let event = absolute("target/event.json");
    let candidate = absolute("target/candidate.json");
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let invalid = [
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "pull_request",
            "--event",
            event.to_str().unwrap(),
            "--base-sha",
            sha,
            "--candidate-openapi",
            candidate.to_str().unwrap(),
            "--fallback-main-on-invalid-base",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--prefer-marker",
            "--prefer-marker",
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--base-sha",
            sha,
            "--base-sha",
            sha,
        ],
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--release-ref",
            "refs/heads/main",
            "--release-ref",
            "refs/heads/main",
        ],
    ];
    for arguments in invalid {
        assert!(command(&arguments).is_err(), "参数应被拒绝：{arguments:?}");
    }
}

#[test]
fn candidate_plan_alone_declares_api_compilation() {
    let normal = FrontendSourceOptions {
        event_name: FrontendSourceEvent::Push,
        event: None,
        base_sha: None,
        prefer_marker: false,
        candidate_openapi: None,
        release_ref: Some("refs/heads/main".to_owned()),
        fallback_main_on_invalid_base: false,
    };
    let mut non_pull_candidate = normal.clone();
    non_pull_candidate.candidate_openapi = Some(absolute("target/ignored.json"));
    let mut candidate = normal.clone();
    candidate.event_name = FrontendSourceEvent::PullRequest;
    candidate.event = Some(absolute("target/event.json"));
    candidate.base_sha = Some("0123456789abcdef0123456789abcdef01234567".to_owned());
    candidate.candidate_openapi = Some(absolute("target/candidate.json"));

    let normal_plan = ci_execution_plan_for(&CiCommand::FrontendSource(normal)).unwrap();
    let non_pull_plan =
        ci_execution_plan_for(&CiCommand::FrontendSource(non_pull_candidate)).unwrap();
    let candidate_plan = ci_execution_plan_for(&CiCommand::FrontendSource(candidate)).unwrap();
    assert_eq!(
        normal_plan.tasks[0].executor,
        TaskExecutor::CiFrontendSource
    );
    assert_eq!(
        non_pull_plan.tasks[0].executor,
        TaskExecutor::CiFrontendSource
    );
    assert!(
        normal_plan.tasks[0]
            .definition()
            .compilation_coverage
            .is_empty()
    );
    assert_eq!(
        candidate_plan.tasks[0].executor,
        TaskExecutor::CiFrontendCandidateSource
    );
    assert!(
        candidate_plan.tasks[0]
            .definition()
            .compilation_coverage
            .iter()
            .any(|coverage| coverage.contains("export_openapi"))
    );
}
