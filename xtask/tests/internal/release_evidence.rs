use serde_json::{Value, json};

use super::release::{
    evidence::{
        BUSINESS_BACKEND_OUTPUTS, EMPTY_SHA256, Requirement, clean_source, latest_run,
        validate_fixture, validate_pair, validate_run,
    },
    support::sha256,
};

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

fn job() -> Value {
    json!({
        "id": 42,
        "name": "Required",
        "run_id": 123,
        "run_attempt": 2,
        "head_sha": "a".repeat(40),
        "status": "completed",
        "conclusion": "success",
    })
}

fn source(head: &str) -> Value {
    json!({"head": head, "patch_sha256": EMPTY_SHA256, "files": []})
}

fn business_fixture() -> (Value, String) {
    let fixture_sha = sha256(b"business fixture");
    let sources = json!({
        "backend": source(&"a".repeat(40)),
        "frontend": source(&"b".repeat(40)),
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
    let receipt = json!({
        "format_version": 1,
        "fixture": "business",
        "status": "ready",
        "fixture_sha256": fixture_sha,
        "sources": sources,
        "generated": {
            "backend": {
                "head": "a".repeat(40),
                "patch_sha256": sha256(b"generated-backend"),
                "files": files,
            },
            "frontend": {
                "head": "b".repeat(40),
                "patch_sha256": sha256(b"generated-frontend"),
                "files": [{
                    "path": "src/generated/business/order.ts",
                    "sha256": sha256(b"generated frontend order"),
                }],
            },
        },
    });
    (receipt, fixture_sha)
}

#[test]
fn success_keeps_run_attempt_and_job_identity() {
    let receipt = validate_run(&run(), &[job()], &requirement(), 2)
        .unwrap()
        .unwrap();
    assert_eq!(receipt["repository"], "owner/backend");
    assert_eq!(receipt["run_id"], 123);
    assert_eq!(receipt["attempt"], 2);
    assert_eq!(receipt["jobs"][0]["id"], 42);
}

#[test]
fn jobs_endpoint_attempt_is_bound_when_response_omits_it() {
    let mut value = job();
    value.as_object_mut().unwrap().remove("run_attempt");
    assert!(
        validate_run(&run(), &[value.clone()], &requirement(), 2)
            .unwrap()
            .is_some()
    );
    assert!(validate_run(&run(), &[value], &requirement(), 1).is_err());
}

#[test]
fn all_unsuccessful_results_fail_closed() {
    for outcome in [
        json!("failure"),
        json!("cancelled"),
        json!("timed_out"),
        json!("skipped"),
        json!("neutral"),
        Value::Null,
    ] {
        let mut value = run();
        value["conclusion"] = outcome.clone();
        assert!(
            validate_run(&value, &[job()], &requirement(), 2).is_err(),
            "{outcome}"
        );
    }
}

#[test]
fn required_jobs_must_be_unique_current_and_successful() {
    assert!(validate_run(&run(), &[job(), job()], &requirement(), 2).is_err());
    for (field, value) in [
        ("head_sha", json!("b".repeat(40))),
        ("run_attempt", json!(1)),
        ("run_id", json!(99)),
        ("conclusion", json!("skipped")),
        ("conclusion", json!("failure")),
    ] {
        let mut changed = job();
        changed[field] = value;
        assert!(
            validate_run(&run(), &[changed], &requirement(), 2).is_err(),
            "{field}"
        );
    }
}

#[test]
fn stale_jobs_and_pending_runs_wait() {
    assert!(
        validate_run(&run(), &[], &requirement(), 2)
            .unwrap()
            .is_none()
    );
    let mut pending = job();
    pending["conclusion"] = Value::Null;
    assert!(
        validate_run(&run(), &[pending], &requirement(), 2)
            .unwrap()
            .is_none()
    );
    for status in ["queued", "in_progress", "waiting", "pending", "requested"] {
        let mut value = run();
        value["status"] = json!(status);
        assert!(
            validate_run(&value, &[], &requirement(), 2)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn wrong_sha_or_tag_is_rejected() {
    let mut value = run();
    value["head_sha"] = json!("b".repeat(40));
    assert!(validate_run(&value, &[job()], &requirement(), 2).is_err());
    let mut tagged = requirement();
    tagged.tag = Some("v0.13.1".to_owned());
    assert!(latest_run(&[run()], &tagged).is_none());
    assert!(validate_run(&run(), &[job()], &tagged, 2).is_err());
}

#[test]
fn latest_failed_attempt_never_hides_behind_old_success() {
    let mut old = run();
    old["run_attempt"] = json!(1);
    let mut failed = run();
    failed["conclusion"] = json!("failure");
    assert_eq!(
        latest_run(&[old, failed.clone()], &requirement()).unwrap(),
        &failed
    );
    let mut newer = failed;
    newer["id"] = json!(124);
    newer["run_number"] = json!(6);
    newer["run_attempt"] = json!(1);
    assert_eq!(
        latest_run(&[run(), newer.clone()], &requirement()).unwrap(),
        &newer
    );
}

#[test]
fn pair_receipt_is_bound_to_run_attempt_and_exact_clean_sources() {
    let evidence = validate_run(&run(), &[job()], &requirement(), 2)
        .unwrap()
        .unwrap();
    let mut receipt = json!({
        "format_version": 1,
        "backend_sha": "a".repeat(40),
        "frontend_sha": "b".repeat(40),
        "run_id": 123,
        "attempt": 2,
        "sources": {
            "backend": source(&"a".repeat(40)),
            "frontend": source(&"b".repeat(40)),
        },
    });
    validate_pair(&receipt, &evidence, &"a".repeat(40), &"b".repeat(40)).unwrap();
    for (field, value) in [
        ("attempt", json!(1)),
        ("run_id", json!(124)),
        ("frontend_sha", json!("c".repeat(40))),
    ] {
        let mut changed = receipt.clone();
        changed[field] = value;
        assert!(validate_pair(&changed, &evidence, &"a".repeat(40), &"b".repeat(40)).is_err());
    }
    receipt["sources"]["frontend"]["patch_sha256"] = json!("c".repeat(64));
    assert!(validate_pair(&receipt, &evidence, &"a".repeat(40), &"b".repeat(40)).is_err());
    assert!(!clean_source(
        receipt
            .get("sources")
            .and_then(|value| value.get("frontend")),
        &"b".repeat(40)
    ));
}

#[test]
fn business_fixture_requires_clean_sources_and_completed_generation() {
    let (receipt, fixture_sha) = business_fixture();
    validate_fixture(
        Some(&receipt),
        &"a".repeat(40),
        &"b".repeat(40),
        &fixture_sha,
    )
    .unwrap();
    for (field, value) in [
        ("status", json!("preparing")),
        ("fixture", json!("core")),
        ("fixture_sha256", json!(EMPTY_SHA256)),
        ("sources", Value::Null),
        ("generated", json!({})),
    ] {
        let mut changed = receipt.clone();
        changed[field] = value;
        assert!(
            validate_fixture(
                Some(&changed),
                &"a".repeat(40),
                &"b".repeat(40),
                &fixture_sha
            )
            .is_err()
        );
    }
    assert!(
        validate_fixture(
            Some(&receipt),
            &"a".repeat(40),
            &"b".repeat(40),
            &"d".repeat(64)
        )
        .is_err()
    );
}

#[test]
fn business_fixture_binds_outputs_and_requires_generated_frontend() {
    let (receipt, fixture_sha) = business_fixture();
    let mut missing = receipt.clone();
    missing["generated"]["backend"]["files"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(
        validate_fixture(
            Some(&missing),
            &"a".repeat(40),
            &"b".repeat(40),
            &fixture_sha
        )
        .is_err()
    );

    let mut unchanged = receipt.clone();
    unchanged["generated"]["backend"] = unchanged["sources"]["backend"].clone();
    assert!(
        validate_fixture(
            Some(&unchanged),
            &"a".repeat(40),
            &"b".repeat(40),
            &fixture_sha
        )
        .is_err()
    );

    let mut wrong_model = receipt.clone();
    for entry in wrong_model["generated"]["backend"]["files"]
        .as_array_mut()
        .unwrap()
    {
        if entry["path"] == "crates/order-business/src/resources/mod.rs" {
            entry["sha256"] = json!("c".repeat(64));
        }
    }
    assert!(
        validate_fixture(
            Some(&wrong_model),
            &"a".repeat(40),
            &"b".repeat(40),
            &fixture_sha
        )
        .is_err()
    );

    let mut old_fixture = receipt.clone();
    old_fixture["generated"]["frontend"] = old_fixture["sources"]["frontend"].clone();
    let failure = validate_fixture(
        Some(&old_fixture),
        &"a".repeat(40),
        &"b".repeat(40),
        &fixture_sha,
    )
    .unwrap_err();
    assert!(failure.to_string().contains("frontend 没有实际生成变化"));

    let mut wrong_digest = receipt;
    wrong_digest["generated"]["frontend"]["files"][0]["sha256"] = json!("invalid");
    assert!(
        validate_fixture(
            Some(&wrong_digest),
            &"a".repeat(40),
            &"b".repeat(40),
            &fixture_sha
        )
        .is_err()
    );
}
