use std::{
    cell::RefCell,
    collections::VecDeque,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use super::release::{
    evidence::{BUSINESS_BACKEND_OUTPUTS, EMPTY_SHA256, Requirement},
    github::{Api, redact_diagnostic},
    release_ci::{
        artifacts::{ARTIFACT_ARCHIVE_MAX_BYTES, select_artifacts},
        collect::{
            api_timeout, await_evidence_with, collect, coordinated_evidence_with, pages,
            validate_remote_tags,
        },
        failure_receipt,
    },
    support::{sha256, write_new_json},
};

struct FakeApi {
    responses: RefCell<VecDeque<std::result::Result<Value, String>>>,
    calls: RefCell<Vec<String>>,
}

impl FakeApi {
    fn new(responses: Vec<Value>) -> Self {
        Self {
            responses: RefCell::new(responses.into_iter().map(Ok).collect()),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Api for FakeApi {
    fn get(&self, endpoint: &str, _timeout: Duration) -> crate::Result<Value> {
        self.calls.borrow_mut().push(endpoint.to_owned());
        self.responses
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Err("缺少测试响应".to_owned()))
            .map_err(Into::into)
    }

    fn download(
        &self,
        _endpoint: &str,
        _destination: &std::path::Path,
        _timeout: Duration,
    ) -> crate::Result<()> {
        Err("测试未配置下载".into())
    }
}

fn requirement() -> Requirement {
    Requirement {
        repository: "owner/backend".to_owned(),
        workflow: "ci.yml".to_owned(),
        sha: "a".repeat(40),
        jobs: vec!["Required".to_owned()],
        tag: Some("v0.13.0".to_owned()),
    }
}

fn run() -> Value {
    json!({
        "id": 123,
        "run_number": 5,
        "run_attempt": 2,
        "head_sha": "a".repeat(40),
        "status": "completed",
        "conclusion": "success",
        "event": "push",
        "head_branch": "v0.13.0",
    })
}

fn changed_run(changes: Value) -> Value {
    let mut value = run();
    for (key, changed) in changes.as_object().unwrap() {
        value[key] = changed.clone();
    }
    value
}

fn job() -> Value {
    json!({
        "id": 42,
        "name": "Required",
        "run_id": 123,
        "head_sha": "a".repeat(40),
        "status": "completed",
        "conclusion": "success",
    })
}

fn collection_responses(current: Value, newest: Value) -> Vec<Value> {
    vec![
        json!({"workflow_runs": [run()]}),
        run(),
        json!({"jobs": [job()]}),
        current,
        json!({"workflow_runs": [newest]}),
    ]
}

#[test]
fn release_requirements_bind_both_workflows_to_the_tag() {
    use super::cli::{GitSha, ReleaseCiEvidenceOptions, ReleaseTag, RepositorySlug};
    use super::release::release_ci::requirements;

    let options = ReleaseCiEvidenceOptions {
        backend_repository: RepositorySlug::parse("owner/backend", "test").unwrap(),
        frontend_repository: RepositorySlug::parse("owner/frontend", "test").unwrap(),
        backend_sha: GitSha::parse(&"a".repeat(40), "test").unwrap(),
        frontend_sha: GitSha::parse(&"b".repeat(40), "test").unwrap(),
        tag: ReleaseTag::parse("v0.13.0").unwrap(),
        timeout_seconds: 5,
        output: std::env::temp_dir().join("release-ci-test.json"),
        tag_oids: None,
    };
    let requirements = requirements(&options);
    assert_eq!(requirements.len(), 2);
    assert_eq!(requirements[0].workflow, "ci.yml");
    assert_eq!(requirements[1].workflow, "extended-ci.yml");
    assert!(
        requirements[1]
            .jobs
            .iter()
            .any(|job| job == "Real API MySQL Redis full-stack build")
    );
    assert!(
        requirements[1]
            .jobs
            .iter()
            .any(|job| job == "Generated business crate data migration build")
    );
    assert!(
        requirements[1]
            .jobs
            .iter()
            .any(|job| job == "Linux DevEx Cgroup Memory")
    );
    assert!(
        requirements
            .iter()
            .all(|requirement| requirement.tag.as_deref() == Some("v0.13.0"))
    );
}

#[test]
fn attempt_specific_collection_accepts_real_job_shape() {
    let api = FakeApi::new(collection_responses(run(), run()));
    let result = collect(
        &api,
        &requirement(),
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap()
    .unwrap();
    assert_eq!(result["attempt"], 2);
    assert!(api.calls.borrow()[2].contains("/runs/123/attempts/2/jobs?"));
}

#[test]
fn attempt_change_or_new_run_during_collection_restarts_wait() {
    for (current, newest) in [
        (
            changed_run(json!({"run_attempt": 3, "status": "queued"})),
            run(),
        ),
        (
            run(),
            changed_run(json!({"id": 124, "run_number": 6, "status": "queued"})),
        ),
    ] {
        let api = FakeApi::new(collection_responses(current, newest));
        assert!(
            collect(
                &api,
                &requirement(),
                Instant::now() + Duration::from_secs(5)
            )
            .unwrap()
            .is_none()
        );
    }
}

#[test]
fn wrong_run_id_fails_before_loading_jobs() {
    let api = FakeApi::new(vec![
        json!({"workflow_runs": [run()]}),
        changed_run(json!({"id": 999})),
    ]);
    assert!(
        collect(
            &api,
            &requirement(),
            Instant::now() + Duration::from_secs(5)
        )
        .is_err()
    );
    assert_eq!(api.calls.borrow().len(), 2);
}

#[test]
fn pagination_does_not_truncate_at_hundred_items() {
    let api = FakeApi::new(vec![
        json!({"jobs": vec![job(); 100]}),
        json!({"jobs": [job()]}),
    ]);
    assert_eq!(
        pages(
            &api,
            "repos/owner/backend/actions/runs/123/jobs",
            "jobs",
            Instant::now() + Duration::from_secs(5)
        )
        .unwrap()
        .len(),
        101
    );
    assert!(api.calls.borrow()[0].contains("page=1"));
    assert!(api.calls.borrow()[1].contains("page=2"));
}

#[test]
fn timeout_waits_then_returns_completed_evidence() {
    let base = Instant::now();
    let times = RefCell::new(VecDeque::from([0_u64, 2]));
    let result = await_evidence_with(
        &[requirement()],
        base + Duration::from_secs(1),
        |_| Ok(None),
        || base + Duration::from_secs(times.borrow_mut().pop_front().unwrap_or(2)),
        |_| {},
    );
    assert!(result.unwrap_err().to_string().contains("超时"));

    let responses = RefCell::new(VecDeque::from([
        None,
        Some(json!({"run_id": 123, "attempt": 2})),
    ]));
    let result = await_evidence_with(
        &[requirement()],
        base + Duration::from_secs(3),
        |_| Ok(responses.borrow_mut().pop_front().unwrap_or(None)),
        || base,
        |_| {},
    )
    .unwrap();
    assert_eq!(result[0]["run_id"], 123);
}

#[test]
fn remote_tags_dereference_nested_annotations_and_detect_moves() {
    let required = vec![
        Requirement {
            jobs: Vec::new(),
            ..requirement()
        },
        Requirement {
            repository: "owner/frontend".to_owned(),
            workflow: "ci.yml".to_owned(),
            sha: "b".repeat(40),
            jobs: Vec::new(),
            tag: Some("v0.13.0".to_owned()),
        },
    ];
    let tag_oids = ("c".repeat(40), "d".repeat(40));
    let nested = "e".repeat(40);
    let reference = |oid: &str| {
        json!({
            "ref": "refs/tags/v0.13.0",
            "object": {"type": "tag", "sha": oid},
        })
    };
    let api = FakeApi::new(vec![
        reference(&tag_oids.0),
        reference(&tag_oids.1),
        json!({"sha": tag_oids.0, "object": {"type": "tag", "sha": nested}}),
        json!({"sha": nested, "object": {"type": "commit", "sha": required[0].sha}}),
        json!({"sha": tag_oids.1, "object": {"type": "commit", "sha": required[1].sha}}),
        reference(&tag_oids.0),
        reference(&tag_oids.1),
    ]);
    let evidence = validate_remote_tags(
        &api,
        &required,
        &tag_oids,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(evidence[0]["commit"], required[0].sha);

    let moved = FakeApi::new(vec![
        reference(&tag_oids.0),
        reference(&tag_oids.1),
        json!({"sha": tag_oids.0, "object": {"type": "commit", "sha": required[0].sha}}),
        json!({"sha": tag_oids.1, "object": {"type": "commit", "sha": required[1].sha}}),
        reference(&tag_oids.0),
        reference(&"f".repeat(40)),
    ]);
    assert!(
        validate_remote_tags(
            &moved,
            &required,
            &tag_oids,
            Instant::now() + Duration::from_secs(5)
        )
        .is_err()
    );
}

#[test]
fn artifact_selection_requires_both_current_unexpired_roles() {
    let artifacts = vec![
        json!({"id": 10, "name": "ryframe-full-stack-123-2", "size_in_bytes": 10}),
        json!({"id": 11, "name": "ryframe-full-stack-123-2-business", "size_in_bytes": 20}),
    ];
    assert_eq!(select_artifacts(&artifacts, 123, 2).unwrap().len(), 2);
    for invalid in [
        artifacts[..1].to_vec(),
        vec![
            artifacts[0].clone(),
            changed_artifact(&artifacts[1], json!({"expired": true})),
        ],
        vec![
            artifacts[0].clone(),
            changed_artifact(&artifacts[1], json!({"name": "wrong"})),
        ],
        vec![
            artifacts[0].clone(),
            artifacts[1].clone(),
            artifacts[1].clone(),
        ],
        vec![
            artifacts[0].clone(),
            changed_artifact(
                &artifacts[1],
                json!({"size_in_bytes": ARTIFACT_ARCHIVE_MAX_BYTES + 1}),
            ),
        ],
    ] {
        assert!(select_artifacts(&invalid, 123, 2).is_err());
    }
}

fn changed_artifact(value: &Value, changes: Value) -> Value {
    let mut value = value.clone();
    for (key, changed) in changes.as_object().unwrap() {
        value[key] = changed.clone();
    }
    value
}

fn pair_fixture(evidence: &Value) -> Value {
    let fixture_sha = sha256(b"business fixture");
    let sources = json!({
        "backend": {"head": "a".repeat(40), "patch_sha256": EMPTY_SHA256, "files": []},
        "frontend": {"head": "b".repeat(40), "patch_sha256": EMPTY_SHA256, "files": []},
    });
    let files = BUSINESS_BACKEND_OUTPUTS
        .iter()
        .map(|path| {
            json!({
                "path": path,
                "sha256": if *path == "crates/order-business/src/resources/mod.rs" {
                    fixture_sha.clone()
                } else {
                    sha256(path.as_bytes())
                },
            })
        })
        .collect::<Vec<_>>();
    let pair = json!({
        "format_version": 1,
        "backend_sha": "a".repeat(40),
        "frontend_sha": "b".repeat(40),
        "run_id": evidence["run_id"],
        "attempt": evidence["attempt"],
        "sources": sources,
    });
    let fixture = json!({
        "format_version": 1,
        "fixture": "business",
        "status": "ready",
        "fixture_sha256": fixture_sha,
        "sources": sources,
        "generated": {
            "backend": {
                "head": "a".repeat(40),
                "patch_sha256": sha256(b"generated"),
                "files": files,
            },
            "frontend": sources["frontend"],
        },
    });
    let mut business = pair.clone();
    business["fixture"] = fixture;
    json!({"core": pair, "business": business})
}

fn coordination_run(requirement: &Requirement) -> Value {
    let (run_id, attempt) = match requirement.workflow.as_str() {
        "ci.yml" => (11, 1),
        "extended-ci.yml" => (22, 3),
        _ => (33, 4),
    };
    json!({
        "repository": requirement.repository,
        "workflow": requirement.workflow,
        "sha": requirement.sha,
        "run_id": run_id,
        "attempt": attempt,
    })
}

#[test]
fn coordination_rechecks_ci_after_reading_pair_artifacts() {
    let required = [
        requirement(),
        Requirement {
            workflow: "extended-ci.yml".to_owned(),
            ..requirement()
        },
    ];
    let receipts = pair_fixture(&coordination_run(&required[1]));
    let fixture_sha = receipts["business"]["fixture"]["fixture_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let base = Instant::now();
    let events = RefCell::new(Vec::new());
    let result = coordinated_evidence_with(
        base + Duration::from_secs(3),
        &required,
        &"b".repeat(40),
        &fixture_sha,
        None,
        None,
        |requirement| {
            events.borrow_mut().push("ci");
            Ok(Some(coordination_run(requirement)))
        },
        |_, _, _, _| {
            events.borrow_mut().push("pairs");
            Ok(receipts.clone())
        },
        || base,
        |_| {},
        |_, _| Ok(Value::Null),
    )
    .unwrap();
    assert_eq!(
        events.borrow().as_slice(),
        ["ci", "ci", "pairs", "ci", "ci"]
    );
    assert_eq!(
        result["source_pairs"]["business"]["fixture"]["fixture"],
        "business"
    );
}

#[test]
fn recheck_after_pair_artifacts_cannot_hide_a_new_failure() {
    let required = [
        requirement(),
        Requirement {
            workflow: "extended-ci.yml".to_owned(),
            ..requirement()
        },
    ];
    let receipts = pair_fixture(&coordination_run(&required[1]));
    let fixture_sha = receipts["business"]["fixture"]["fixture_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    let calls = RefCell::new(0_u32);
    let error = coordinated_evidence_with(
        Instant::now() + Duration::from_secs(3),
        &required,
        &"b".repeat(40),
        &fixture_sha,
        None,
        None,
        |requirement| {
            let mut calls = calls.borrow_mut();
            *calls += 1;
            if *calls <= 2 {
                Ok(Some(coordination_run(requirement)))
            } else {
                Err("最新重跑失败".into())
            }
        },
        |_, _, _, _| Ok(receipts.clone()),
        Instant::now,
        |_| {},
        |_, _| Ok(Value::Null),
    )
    .unwrap_err();
    assert!(error.to_string().contains("最新重跑失败"));
}

#[test]
fn full_stack_receipts_are_read_from_the_extended_run() {
    let mut core = requirement();
    core.sha = "c".repeat(40);
    let mut extended = requirement();
    extended.workflow = "extended-ci.yml".to_owned();
    let extra = Requirement {
        workflow: "audit.yml".to_owned(),
        sha: "d".repeat(40),
        ..requirement()
    };
    let extended_run = coordination_run(&extended);
    let receipts = pair_fixture(&extended_run);
    let fixture_sha = receipts["business"]["fixture"]["fixture_sha256"]
        .as_str()
        .unwrap()
        .to_owned();
    for required in [
        vec![core.clone(), extended.clone()],
        vec![extended.clone(), core.clone()],
        vec![extra.clone(), core.clone(), extended.clone()],
        vec![core.clone(), extended.clone(), extra.clone()],
        vec![extended.clone(), extra, core],
    ] {
        let observed = RefCell::new(Vec::new());
        let result = coordinated_evidence_with(
            Instant::now() + Duration::from_secs(3),
            &required,
            &"b".repeat(40),
            &fixture_sha,
            None,
            None,
            |requirement| Ok(Some(coordination_run(requirement))),
            |evidence, backend_sha, _, _| {
                observed
                    .borrow_mut()
                    .push((evidence.clone(), backend_sha.to_owned()));
                Ok(receipts.clone())
            },
            Instant::now,
            |_| {},
            |_, _| Ok(Value::Null),
        )
        .unwrap();
        assert_eq!(
            *observed.borrow(),
            [(extended_run.clone(), extended.sha.clone())]
        );
        assert_eq!(
            result["runs"],
            json!(required.iter().map(coordination_run).collect::<Vec<_>>())
        );
        assert_eq!(result["source_pairs"], receipts);
    }
}

#[test]
fn missing_or_duplicate_extended_workflow_fails_before_reading_artifacts() {
    let extended = Requirement {
        workflow: "extended-ci.yml".to_owned(),
        ..requirement()
    };
    for (required, reason) in [
        (vec![requirement(), requirement()], "缺少 extended-ci.yml"),
        (
            vec![requirement(), extended.clone(), extended],
            "重复的 extended-ci.yml",
        ),
    ] {
        let read_pairs = RefCell::new(false);
        let error = coordinated_evidence_with(
            Instant::now() + Duration::from_secs(3),
            &required,
            &"b".repeat(40),
            &sha256(b"business fixture"),
            None,
            None,
            |requirement| Ok(Some(coordination_run(requirement))),
            |_, _, _, _| {
                *read_pairs.borrow_mut() = true;
                Ok(Value::Null)
            },
            Instant::now,
            |_| {},
            |_, _| Ok(Value::Null),
        )
        .unwrap_err();
        assert!(error.to_string().contains(reason), "{error}");
        assert!(!*read_pairs.borrow());
    }
}

#[test]
fn remote_tag_cycles_and_excessive_nesting_fail_closed() {
    let required = vec![
        Requirement {
            jobs: Vec::new(),
            ..requirement()
        },
        Requirement {
            repository: "owner/frontend".to_owned(),
            workflow: "ci.yml".to_owned(),
            sha: "b".repeat(40),
            jobs: Vec::new(),
            tag: Some("v0.13.0".to_owned()),
        },
    ];
    let tag_oids = ("c".repeat(40), "d".repeat(40));
    let nested = "e".repeat(40);
    let reference = |oid: &str| {
        json!({
            "ref": "refs/tags/v0.13.0",
            "object": {"type": "tag", "sha": oid},
        })
    };
    let cycle = FakeApi::new(vec![
        reference(&tag_oids.0),
        reference(&tag_oids.1),
        json!({"sha": tag_oids.0, "object": {"type": "tag", "sha": nested}}),
        json!({"sha": nested, "object": {"type": "tag", "sha": tag_oids.0}}),
    ]);
    assert!(
        validate_remote_tags(
            &cycle,
            &required,
            &tag_oids,
            Instant::now() + Duration::from_secs(5),
        )
        .is_err()
    );

    let chain = (1..=8)
        .map(|value| format!("{value:040x}"))
        .collect::<Vec<_>>();
    let mut responses = vec![reference(&tag_oids.0), reference(&tag_oids.1)];
    let mut current = tag_oids.0.clone();
    for target in chain {
        responses.push(json!({
            "sha": current,
            "object": {"type": "tag", "sha": target},
        }));
        current = format!("{target:0>40}");
    }
    let deep = FakeApi::new(responses);
    let error = validate_remote_tags(
        &deep,
        &required,
        &tag_oids,
        Instant::now() + Duration::from_secs(5),
    )
    .unwrap_err();
    assert!(error.to_string().contains("解引用层数超限"));
}

#[test]
fn source_pair_receipt_requires_clean_distinct_git_roots() {
    use std::process::Command;

    let root = std::env::temp_dir().join(format!("ryframe-release-pair-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let backend = root.join("backend");
    let frontend = root.join("frontend");
    for repository in [&backend, &frontend] {
        std::fs::create_dir_all(repository).unwrap();
        std::fs::write(repository.join("README.md"), "fixture\n").unwrap();
        for arguments in [
            vec!["init", "--quiet"],
            vec!["add", "README.md"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(arguments)
                    .current_dir(repository)
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }
    let receipt = super::release::archive::source_pair_receipt(&backend, &frontend, 12, 3).unwrap();
    assert_eq!(receipt["run_id"], 12);
    assert_eq!(receipt["attempt"], 3);
    let receipt_path = root.join("source-pair.json");
    write_new_json(&receipt_path, &receipt).unwrap();
    let before = std::fs::read(&receipt_path).unwrap();
    assert_eq!(
        super::release::archive::verify_source_pair_receipt(
            &receipt_path,
            &backend,
            &frontend,
            12,
            3,
        )
        .unwrap(),
        receipt
    );
    assert!(
        super::release::archive::verify_source_pair_receipt(
            &receipt_path,
            &backend,
            &frontend,
            12,
            4,
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&receipt_path).unwrap(), before);
    assert!(super::release::archive::source_pair_receipt(&backend, &backend, 12, 3).is_err());
    std::fs::write(backend.join("dirty.txt"), "dirty\n").unwrap();
    assert!(super::release::archive::source_pair_receipt(&backend, &frontend, 12, 3).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn github_diagnostics_redact_tokens_headers_and_url_credentials() {
    let redacted = redact_diagnostic(
        "Authorization: Bearer secret-value token=plain \
         ghp_abcdefghijklmnopqrstuvwxyz https://user:pass@example.invalid/path",
    );
    for secret in [
        "secret-value",
        "plain",
        "ghp_abcdefghijklmnopqrstuvwxyz",
        "user:pass",
    ] {
        assert!(!redacted.contains(secret), "{redacted}");
    }
    assert!(redacted.contains("[REDACTED]"));
}

#[test]
fn collection_failure_receipt_keeps_source_and_tag_identity() {
    use super::cli::{GitSha, ReleaseCiEvidenceOptions, ReleaseTag, RepositorySlug};

    let options = ReleaseCiEvidenceOptions {
        backend_repository: RepositorySlug::parse("owner/backend", "test").unwrap(),
        frontend_repository: RepositorySlug::parse("owner/frontend", "test").unwrap(),
        backend_sha: GitSha::parse(&"a".repeat(40), "test").unwrap(),
        frontend_sha: GitSha::parse(&"b".repeat(40), "test").unwrap(),
        tag: ReleaseTag::parse("v0.13.0").unwrap(),
        timeout_seconds: 5,
        output: std::env::temp_dir().join("release-ci-failure.json"),
        tag_oids: None,
    };
    let receipt = failure_receipt(
        &options,
        Some(&("c".repeat(40), "d".repeat(40))),
        "stream failed token=[REDACTED]",
    );
    assert_eq!(receipt["status"], "failed");
    assert_eq!(receipt["backend_sha"], "a".repeat(40));
    assert_eq!(receipt["frontend_sha"], "b".repeat(40));
    assert_eq!(receipt["backend_tag_oid"], "c".repeat(40));
    assert_eq!(receipt["frontend_tag_oid"], "d".repeat(40));
}

#[test]
fn deadline_is_shared_and_capped_at_one_minute() {
    let now = Instant::now();
    assert_eq!(
        api_timeout(now + Duration::from_secs(120), now).unwrap(),
        Duration::from_secs(60)
    );
    assert_eq!(
        api_timeout(now + Duration::from_secs(5), now).unwrap(),
        Duration::from_secs(5)
    );
    assert!(api_timeout(now, now).is_err());
}

#[test]
fn pair_receipt_writer_never_overwrites_an_existing_receipt() {
    let root = std::env::temp_dir().join(format!(
        "ryframe-release-pair-writer-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source-pair.json");
    let receipt = json!({"format_version": 1, "run_id": 12, "attempt": 3});
    write_new_json(&path, &receipt).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(write_new_json(&path, &json!({"changed": true})).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    let _ = std::fs::remove_dir_all(root);
}
