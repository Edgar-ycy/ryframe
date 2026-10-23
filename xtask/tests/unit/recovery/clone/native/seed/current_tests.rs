use super::{Current, fixture::Fixture};
use crate::recovery::clone::native::{
    json as document,
    lifecycle::{lock::RunLock, state},
    resources::common,
};
use serde_json::json;
use std::fs;

fn running(mode: &str) -> Fixture {
    let mut fixture = Fixture::new();
    fixture.register();
    let owner = RunLock::claim(&fixture.run).unwrap();
    let (number, attempt) = state::begin(&fixture.run, "seed-runtime", mode, &json!({})).unwrap();
    state::bind_controller(&fixture.run, number, &owner.owner, &attempt).unwrap();
    fixture.owner = Some(owner);
    fixture
}

#[test]
fn producer_current_proof_preserves_legitimate_business_evolution_and_never_creates_ledger() {
    let fixture = running("identities-apply");
    let proof = Current::load(&fixture.backend, &fixture.run).unwrap();
    fixture.file(
        "business-after",
        &json!({"users":200,"delivered_outbox":123}),
    );
    proof.check(&fixture.backend).unwrap();
    assert!(!fixture.run.join("seed-runtime/identities").exists());
}

#[test]
fn producer_current_proof_rejects_private_plan_api_configuration_and_raw_post_drift() {
    for name in [
        "private.json",
        "identity-plan.json",
        "post-copy.json",
        "post-copy/runtime/runtime.json",
        "post-copy/runtime/api.json",
        "historical.json",
    ] {
        let fixture = running("identities-verify");
        let proof = Current::load(&fixture.backend, &fixture.run).unwrap();
        fs::write(
            fixture.run.join(name),
            "{\"password\":\"untrusted-secret\"}",
        )
        .unwrap();
        assert!(proof.check(&fixture.backend).is_err(), "{name}");
    }
    let fixture = running("quotas-apply");
    let proof = Current::load(&fixture.backend, &fixture.run).unwrap();
    let config = fixture.old.join("config/app.test.toml");
    fs::write(config, "password = \"untrusted-secret\"\ninvalid").unwrap();
    let error = proof.check(&fixture.backend).unwrap_err().to_string();
    assert!(!error.contains("untrusted-secret"));
}

#[test]
fn producer_current_proof_requires_same_live_controller_and_current_outer_attempt() {
    for drift in ["owner", "controller", "attempt"] {
        let mut fixture = running("departments-apply");
        let proof = Current::load(&fixture.backend, &fixture.run).unwrap();
        let number = state::load(&fixture.run, true).unwrap()["attempts"]
            .as_array()
            .unwrap()
            .len() as u64;
        if drift == "owner" {
            let path = fixture.run.join("run.lock/owner.json");
            let original = document::read(&path).unwrap();
            let mut changed = original.clone();
            changed["identity"]["started"] = json!("1");
            document::write_replace(&path, &changed).unwrap();
            assert!(proof.check(&fixture.backend).is_err());
            document::write_replace(&path, &original).unwrap();
        } else if drift == "controller" {
            let path = state::controller_path(&fixture.run, number);
            let mut record = document::read(&path).unwrap();
            record["attempt_sha256"] = json!("0".repeat(64));
            document::write_replace(&path, &record).unwrap();
            assert!(proof.check(&fixture.backend).is_err());
        } else {
            state::finish(&fixture.run, number, None, Some("Interrupted")).unwrap();
            assert!(proof.check(&fixture.backend).is_err());
        }
        fixture.owner.take().unwrap().release().unwrap();
    }
}

#[test]
fn current_proof_never_treats_failed_or_cleanup_stage_as_mutation_authority() {
    for mode in [
        "stop",
        "recover",
        "source-generation-start",
        "register",
        "close",
    ] {
        let fixture = running(mode);
        assert!(
            Current::load(&fixture.backend, &fixture.run).is_err(),
            "{mode}"
        );
    }
    let mut fixture = Fixture::new();
    fixture.register();
    let before = common::binding(&fixture.run.join("seed-runtime.json")).unwrap();
    assert!(Current::load(&fixture.backend, &fixture.run).is_err());
    assert_eq!(
        before,
        common::binding(&fixture.run.join("seed-runtime.json")).unwrap()
    );
    assert!(!fixture.run.join("seed-runtime/identities").exists());
}
