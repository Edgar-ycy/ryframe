use std::path::PathBuf;

use super::*;

const RUNNER_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VERIFIER_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const COORDINATOR_SHA: &str = "cccccccccccccccccccccccccccccccccccccccc";
const STARTED: &str = "2026-11-08T10:00:01.000Z";
const COMPLETED: &str = "2026-11-08T10:02:00.000Z";
const FRONTEND: &str = "http://127.0.0.1:4174";
const SCENARIOS: [&str; 9] = [
    "login",
    "session",
    "post",
    "notice",
    "tenant",
    "export",
    "message",
    "schedule",
    "restored-data",
];

struct Fixture {
    coordinator: PathBuf,
    runner: PathBuf,
    verifier: PathBuf,
    record: Value,
    runtime_authority: Value,
    source_generation: Value,
    dataset_lineage: Value,
    proof: Document,
    tests: Document,
    runtime: Document,
    target: Document,
}

fn doc(path: PathBuf, value: Value) -> Document {
    Document {
        path,
        raw: serde_json::to_vec(&value).unwrap(),
        value,
    }
}

fn descriptor(path: PathBuf, digest: &str) -> Value {
    json!({"path": path, "bytes": 128, "sha256": digest.repeat(64)})
}

impl Fixture {
    fn new() -> Self {
        let root = if cfg!(windows) {
            PathBuf::from(r"D:\fixture")
        } else {
            PathBuf::from("/fixture")
        };
        let runner = root.join("runner");
        let coordinator = root.join("coordinator");
        let verifier = root.join("verifier");
        let output = runner.join(".local-tests").join("playwright-real");
        let runtime = doc(
            output.join("restore-drill-runtime.json"),
            json!({"kind": "runtime"}),
        );
        let target = doc(root.join("target-plan.json"), json!({"kind": "target"}));
        let source_generation = descriptor(root.join("stop.json"), "a");
        let dataset_lineage = descriptor(root.join("lineage.json"), "b");
        let record = json!({
            "plan": {"id": "drill", "scope_id": "restore-scope"},
            "plan_hash": "c".repeat(64),
        });
        let runtime_authority = json!({
            "backup_source_sha": "d".repeat(40),
            "backend_product_sha": "e".repeat(40),
            "backend_execution_sha": "f".repeat(40),
            "backend_adapter_contract": "legacy-stable-readiness-b0-v1",
            "frontend_sha": "1".repeat(40),
            "frontend_endpoint": FRONTEND,
        });
        let tests = doc(
            output.join("restore-drill-tests.json"),
            json!({
                "format_version": 1, "kind": "restore-browser-tests",
                "restore": {"id": "drill", "scope_id": "restore-scope", "plan_hash": "c".repeat(64)},
                "runtime": document::binding(&runtime),
                "target_plan": document::binding(&target),
                "source_generation": source_generation,
                "dataset_lineage": dataset_lineage,
                "sources": {
                    "backup_source_sha": "d".repeat(40),
                    "backend_product_sha": "e".repeat(40),
                    "backend_execution_sha": "f".repeat(40),
                    "backend_adapter_contract": "legacy-stable-readiness-b0-v1",
                    "frontend_sha": "1".repeat(40),
                    "coordinator": {"root": coordinator, "sha": COORDINATOR_SHA},
                    "runner": {"root": runner, "sha": RUNNER_SHA},
                    "verifier": {"root": verifier, "sha": VERIFIER_SHA},
                },
                "frontend_url": FRONTEND, "started_at": STARTED, "completed_at": COMPLETED,
                "runs": [
                    {"title": ["suite", "other test"], "status": "passed", "retry": 0, "scenarios": []},
                    {"title": ["suite", "core"], "status": "passed", "retry": 0, "scenarios": SCENARIOS},
                ],
            }),
        );
        let proof = doc(
            output.join("restore-drill.json"),
            expected(&tests, &runtime, &target),
        );
        Self {
            coordinator,
            runner,
            verifier,
            record,
            runtime_authority,
            source_generation,
            dataset_lineage,
            proof,
            tests,
            runtime,
            target,
        }
    }

    fn check(&self) -> Result<&Value> {
        self.check_sources(self.sources())
    }

    fn sources(&self) -> [Source<'_>; 3] {
        [
            Source {
                root: &self.coordinator,
                sha: COORDINATOR_SHA,
            },
            Source {
                root: &self.runner,
                sha: RUNNER_SHA,
            },
            Source {
                root: &self.verifier,
                sha: VERIFIER_SHA,
            },
        ]
    }

    fn check_sources(&self, [coordinator, runner, verifier]: [Source<'_>; 3]) -> Result<&Value> {
        verify(
            Evidence {
                proof: &self.proof,
                tests: &self.tests,
                runtime: &self.runtime,
                target: &self.target,
            },
            Authority {
                record: &self.record,
                runtime: &self.runtime_authority,
                coordinator,
                runner,
                verifier,
                source_generation: &self.source_generation,
                dataset_lineage: &self.dataset_lineage,
            },
        )
    }

    fn rebind_tests(&mut self) {
        self.tests.raw = serde_json::to_vec_pretty(&self.tests.value).unwrap();
        self.proof.value["tests_receipt_sha256"] = json!(self.tests.sha256());
        self.proof.raw = serde_json::to_vec(&self.proof.value).unwrap();
    }
}

fn expected(tests: &Document, runtime: &Document, target: &Document) -> Value {
    json!({
        "restore_id": "drill",
        "plan_hash": "c".repeat(64),
        "backup_source_sha": "d".repeat(40),
        "backend_product_sha": "e".repeat(40),
        "backend_execution_sha": "f".repeat(40),
        "backend_adapter_contract": "legacy-stable-readiness-b0-v1",
        "frontend_sha": "1".repeat(40),
        "runner_sha": RUNNER_SHA,
        "verifier_sha": VERIFIER_SHA,
        "scope_id": "restore-scope",
        "frontend_url": FRONTEND,
        "runtime_receipt_sha256": runtime.sha256(),
        "tests_receipt_sha256": tests.sha256(),
        "target_plan_sha256": target.sha256(),
        "source_generation_sha256": "a".repeat(64),
        "dataset_lineage_sha256": "b".repeat(64),
        "started_at": STARTED,
        "completed_at": COMPLETED,
        "scenarios": SCENARIOS.map(|name| json!({"name": name, "succeeded": true})),
        "unexpected_console_messages": 0,
        "unexpected_network_failures": 0,
        "axe_serious_or_critical": 0,
    })
}

#[test]
fn exact_b0_and_current_evidence_returns_only_the_original_proof() {
    let mut fixture = Fixture::new();
    assert!(std::ptr::eq(fixture.check().unwrap(), &fixture.proof.value));
    for (field, value) in [
        ("backend_product_sha", json!("f".repeat(40))),
        ("backend_adapter_contract", Value::Null),
    ] {
        fixture.runtime_authority[field] = value.clone();
        fixture.tests.value["sources"][field] = value.clone();
        fixture.proof.value[field] = value;
    }
    fixture.rebind_tests();
    assert!(std::ptr::eq(fixture.check().unwrap(), &fixture.proof.value));
}

#[test]
fn refuses_missing_duplicate_unknown_and_retried_scenarios_even_with_matching_test_digest() {
    for (field, value) in [
        ("scenarios", json!(&SCENARIOS[..8])),
        (
            "scenarios",
            json!([
                "login", "session", "post", "notice", "tenant", "export", "message", "schedule",
                "login"
            ]),
        ),
        (
            "scenarios",
            json!([
                "login", "session", "post", "notice", "tenant", "export", "message", "schedule",
                "other"
            ]),
        ),
        ("retry", json!(1)),
        ("retry", json!(false)),
        ("status", json!("skipped")),
        ("title", json!([" "])),
    ] {
        let mut fixture = Fixture::new();
        fixture.tests.value["runs"][1][field] = value;
        fixture.rebind_tests();
        assert!(fixture.check().is_err(), "应拒绝变更的运行字段 {field}");
    }
}

#[test]
fn refuses_substituted_descriptors_and_raw_document_digests() {
    for (name, field, value) in [
        ("runtime", "bytes", json!(1)),
        ("target_plan", "path", json!("/another/target.json")),
        ("source_generation", "sha256", json!("2".repeat(64))),
        ("dataset_lineage", "bytes", json!(129)),
    ] {
        let mut fixture = Fixture::new();
        fixture.tests.value[name][field] = value;
        fixture.rebind_tests();
        assert!(fixture.check().is_err(), "应拒绝变更的描述符 {name}");
    }
    let mut fixture = Fixture::new();
    fixture.tests.raw.push(b'\n');
    assert!(fixture.check().is_err());
}

#[test]
fn refuses_same_receipts_against_different_independent_source_and_path() {
    let mut fixture = Fixture::new();
    let another = "2".repeat(40);
    let other = fixture.verifier.with_file_name("other-backend");
    for role in 0..3 {
        let mut sources = fixture.sources();
        sources[role].sha = &another;
        assert!(
            fixture.check_sources(sources).is_err(),
            "角色 {role} 的 SHA"
        );
        let mut sources = fixture.sources();
        sources[role].root = &other;
        assert!(fixture.check_sources(sources).is_err(), "角色 {role} 的根");
        let mut sources = fixture.sources();
        sources.swap(role, (role + 1) % 3);
        assert!(fixture.check_sources(sources).is_err(), "角色 {role} 互换");
    }
    fixture.runtime.path.set_file_name("other-runtime.json");
    fixture.tests.value["runtime"] = document::binding(&fixture.runtime);
    fixture.rebind_tests();
    assert!(fixture.check().is_err());
}

#[test]
fn refuses_wrong_proof_fields_and_unzoned_test_times() {
    for (field, value) in [
        ("unexpected_console_messages", json!(1)),
        ("unexpected_network_failures", json!(false)),
        ("axe_serious_or_critical", json!(1)),
        ("dataset_lineage_sha256", json!("2".repeat(64))),
        ("unknown", Value::Null),
    ] {
        let mut fixture = Fixture::new();
        fixture.proof.value[field] = value;
        fixture.proof.raw = serde_json::to_vec(&fixture.proof.value).unwrap();
        assert!(fixture.check().is_err(), "应拒绝变更的证明字段 {field}");
    }
    let mut fixture = Fixture::new();
    fixture.tests.value["started_at"] = json!("2026-11-08T10:00:01.000");
    fixture.proof.value["started_at"] = fixture.tests.value["started_at"].clone();
    fixture.rebind_tests();
    assert!(fixture.check().is_err());
}
