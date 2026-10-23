use std::{fs, path::PathBuf, process::Command as ProcessCommand};

use super::{
    check::TaskExecutor,
    ci,
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
        "--plan",
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
                plan: true,
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
    assert!(!options.plan);
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
        vec![
            "check",
            "ci",
            "frontend-source",
            "--event-name",
            "push",
            "--plan",
            "--plan",
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
        plan: false,
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

fn git(root: &std::path::Path, arguments: &[&str]) -> String {
    let output = ProcessCommand::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn detects_openapi_change_and_selects_the_exact_marker() {
    let repository = tempfile::tempdir().unwrap();
    git(repository.path(), &["init", "--quiet"]);
    git(
        repository.path(),
        &["config", "user.email", "ci@example.invalid"],
    );
    git(repository.path(), &["config", "user.name", "CI"]);
    fs::create_dir(repository.path().join("openapi")).unwrap();
    fs::write(
        repository.path().join("openapi/openapi.json"),
        "{\"openapi\":\"3.1.0\"}\n",
    )
    .unwrap();
    git(repository.path(), &["add", "openapi/openapi.json"]);
    git(repository.path(), &["commit", "--quiet", "-m", "baseline"]);
    let base = git(repository.path(), &["rev-parse", "HEAD"]);
    fs::write(
        repository.path().join("openapi/openapi.json"),
        "{\"openapi\":\"3.1.0\",\"info\":{\"title\":\"changed\"}}\n",
    )
    .unwrap();
    git(repository.path(), &["add", "openapi/openapi.json"]);
    git(repository.path(), &["commit", "--quiet", "-m", "contract"]);

    let marker = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let event = repository.path().join("event.json");
    fs::write(
        &event,
        format!("{{\"pull_request\":{{\"body\":\"Frontend-Commit: {marker}\"}}}}"),
    )
    .unwrap();
    let options = FrontendSourceOptions {
        event_name: FrontendSourceEvent::PullRequest,
        event: Some(event),
        base_sha: Some(base),
        prefer_marker: false,
        candidate_openapi: None,
        release_ref: None,
        fallback_main_on_invalid_base: false,
        plan: false,
    };
    let selected =
        ci::frontend_source::select_ci_frontend_ref(&options, repository.path()).unwrap();
    assert_eq!(selected, (marker.to_ascii_lowercase(), true));
}

#[test]
fn unresolvable_base_falls_back_only_when_explicitly_allowed() {
    let repository = tempfile::tempdir().unwrap();
    let event = repository.path().join("event.json");
    fs::write(&event, "{\"pull_request\":{\"body\":null}}").unwrap();
    let mut options = FrontendSourceOptions {
        event_name: FrontendSourceEvent::PullRequest,
        event: Some(event),
        base_sha: Some("1111111111111111111111111111111111111111".to_owned()),
        prefer_marker: true,
        candidate_openapi: None,
        release_ref: None,
        fallback_main_on_invalid_base: false,
        plan: false,
    };
    assert!(ci::frontend_source::select_ci_frontend_ref(&options, repository.path()).is_err());
    options.fallback_main_on_invalid_base = true;
    assert_eq!(
        ci::frontend_source::select_ci_frontend_ref(&options, repository.path()).unwrap(),
        ("main".to_owned(), false)
    );
}

fn marker_sha(character: char) -> String {
    std::iter::repeat_n(character, 40).collect()
}

#[test]
fn unchanged_contract_uses_main_without_a_marker() {
    assert_eq!(
        ci::frontend_source::select_frontend_ref("Frontend-Commit: invalid", false, false).unwrap(),
        "main"
    );
}

#[test]
fn exact_marker_accepts_case_and_line_endings() {
    let value = marker_sha('A');
    for body in [
        format!("说明\nFrontend-Commit: {value}\n"),
        format!("说明\r\nFrontend-Commit: {value}\r\n"),
        format!("说明\rFrontend-Commit: {value}\r"),
        format!("说明\n  Frontend-Commit: {value}  "),
    ] {
        assert_eq!(
            ci::frontend_source::select_frontend_ref(&body, true, false).unwrap(),
            value.to_ascii_lowercase()
        );
    }
}

#[test]
fn preferred_marker_is_selected_even_when_contract_is_unchanged() {
    let value = marker_sha('B');
    assert_eq!(
        ci::frontend_source::select_frontend_ref(
            &format!("Frontend-Commit: {value}"),
            false,
            true,
        )
        .unwrap(),
        value.to_ascii_lowercase()
    );
    assert_eq!(
        ci::frontend_source::select_frontend_ref("没有配套提交", false, true).unwrap(),
        "main"
    );
}

#[test]
fn invalid_duplicate_and_embedded_markers_fail_closed() {
    let value = marker_sha('1');
    for body in [
        String::new(),
        "Frontend-Commit: abc123".to_owned(),
        format!("Frontend-Commit: {value}\nFrontend-Commit: {value}"),
        format!("- Frontend-Commit: {value}"),
        format!("Frontend-Commit: {value} trailing"),
        format!("Frontend-Commit: {}", marker_sha('1').repeat(2)),
    ] {
        assert!(
            ci::frontend_source::select_frontend_ref(&body, true, false).is_err(),
            "{body}"
        );
    }
    for body in [
        "Frontend-Commit: abc123".to_owned(),
        format!("Frontend-Commit: {}", marker_sha('1').repeat(2)),
    ] {
        assert!(ci::frontend_source::select_frontend_ref(&body, false, true).is_err());
    }
}

#[test]
fn release_refs_accept_only_stable_version_tags() {
    let mut options = FrontendSourceOptions {
        event_name: FrontendSourceEvent::Push,
        event: None,
        base_sha: None,
        prefer_marker: false,
        candidate_openapi: None,
        release_ref: Some("refs/tags/v1.2.3".to_owned()),
        fallback_main_on_invalid_base: false,
        plan: false,
    };
    let root = std::env::current_dir().unwrap();
    assert_eq!(
        ci::frontend_source::select_ci_frontend_ref(&options, &root).unwrap(),
        ("v1.2.3".to_owned(), false)
    );
    options.release_ref = Some("refs/tags/v1.2".to_owned());
    assert_eq!(
        ci::frontend_source::select_ci_frontend_ref(&options, &root).unwrap(),
        ("main".to_owned(), false)
    );
    options.release_ref = None;
    assert_eq!(
        ci::frontend_source::select_ci_frontend_ref(&options, &root).unwrap(),
        ("main".to_owned(), false)
    );
}

#[test]
fn frontend_repository_uses_files_before_github_identity_and_rejects_invalid_values() {
    use ci::frontend_source::select_frontend_repository as select;

    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("config")).unwrap();
    fs::create_dir(root.join(".local-tests")).unwrap();
    assert_eq!(
        select(root, || Some("Example-Owner".into())).unwrap(),
        "Example-Owner/ryframe-vue3"
    );
    let error = select(root, || None).unwrap_err().to_string();
    assert!(error.contains("GITHUB_REPOSITORY_OWNER"));
    assert!(error.contains("workspace.frontend_repository"));
    assert!(error.contains("config/tools.toml"));
    for owner in ["", "bad/owner", "owner\nother", "$(unsafe)"] {
        assert!(select(root, || Some(owner.into())).is_err());
    }
    fs::write(
        root.join("config/tools.toml"),
        "[workspace]\nfrontend_repository='PublicOwner/frontend'\n",
    )
    .unwrap();
    assert_eq!(
        select(root, || panic!("显式文件值不能读取 owner")).unwrap(),
        "PublicOwner/frontend"
    );
    fs::write(
        root.join(".local-tests/tools.toml"),
        "[workspace]\nfrontend_repository='LocalOwner/frontend'\n",
    )
    .unwrap();
    assert_eq!(
        select(root, || panic!("本机文件必须优先")).unwrap(),
        "LocalOwner/frontend"
    );
    for invalid in [
        "",
        "owner",
        "owner/repo/extra",
        "owner/repo\nrepository=other",
        "https://github.com/owner/repo",
        "$(unsafe)/repo",
    ] {
        let value = serde_json::json!({"workspace": {"frontend_repository": invalid}});
        fs::write(
            root.join(".local-tests/tools.toml"),
            toml::to_string(&value).unwrap(),
        )
        .unwrap();
        let error = select(root, || panic!("非法文件值不能回退到 owner"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("workspace.frontend_repository"));
        assert!(error.contains("config/tools.toml"));
        assert!(error.contains(".local-tests/tools.toml"));
        assert!(!error.contains("$(unsafe)"));
    }
}
