use super::{evidence, execution, seed_evidence, test_fixture::Fixture};
use crate::recovery::clone::native::{digest, json as document};
use serde_json::{Value, json};
use std::fs;

fn verified() -> Fixture {
    let mut fixture = Fixture::new();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for mode in ["apply", "verify"] {
        runtime
            .block_on(execution::execute(
                &fixture.plan,
                &fixture.root(),
                mode,
                &uuid::Uuid::new_v4().to_string(),
                &mut fixture.context,
                &mut |_| Ok(()),
            ))
            .unwrap();
    }
    fixture
}
fn verify(fixture: &Fixture) -> crate::Result<Value> {
    seed_evidence::verify(
        &crate::workspace::root_dir(),
        &fixture.plan,
        &fixture.root(),
    )
}

#[test]
fn published_identity_evidence_binds_full_622_entries_200_users_and_ten_templates_without_writes() {
    let fixture = verified();
    let root = fixture.root();
    let before = fs::read(root.join("ledger.json")).unwrap();
    let writes = fixture.context.data.borrow().writes;
    let value = verify(&fixture).unwrap();
    assert_eq!(value["templates"].as_array().unwrap().len(), 10);
    assert_eq!(fixture.ledger()["entries"].as_array().unwrap().len(), 622);
    assert_eq!(fixture.ledger()["users"].as_object().unwrap().len(), 200);
    assert_eq!(before, fs::read(root.join("ledger.json")).unwrap());
    assert_eq!(writes, fixture.context.data.borrow().writes);
    assert_eq!(verify(&fixture).unwrap(), value);
    for secret in [
        "Fixture-Database!123",
        "Fixture-Admin!123",
        "Fixture-User!123",
        "fixture-secret",
    ] {
        assert!(!value.to_string().contains(secret));
    }
}

#[test]
fn publication_requires_exact_inventory_nonce_root_ledger_and_complete_receipt_projection() {
    let fixture = verified();
    let root = fixture.root();
    let (intent_name, receipt_name) =
        seed_evidence::names(fixture.plan["plan_sha256"].as_str().unwrap(), &root);
    let intent_path = root.join(intent_name);
    let receipt_path = root.join(receipt_name);
    let original_intent = fs::read(&intent_path).unwrap();
    let original_receipt = fs::read(&receipt_path).unwrap();
    for field in [
        "publication_nonce",
        "identity_state",
        "ledger_sha256",
        "receipt_file",
        "receipt_sha256",
    ] {
        let mut value = evidence::parse(&original_intent).unwrap();
        value[field] = json!("different");
        document::write_replace(&intent_path, &value).unwrap();
        assert!(verify(&fixture).is_err(), "{field}");
        fs::write(&intent_path, &original_intent).unwrap();
    }
    for field in [
        "identities",
        "identity_pools",
        "templates",
        "message_audience",
        "audience_sha256",
        "homepage_user_slots",
    ] {
        let mut receipt = evidence::parse(&original_receipt).unwrap();
        receipt[field] = Value::Null;
        document::write_replace(&receipt_path, &receipt).unwrap();
        let mut intent = evidence::parse(&original_intent).unwrap();
        intent["receipt"] = receipt;
        intent["receipt_sha256"] = json!(digest::file(&receipt_path).unwrap().sha256);
        document::write_replace(&intent_path, &intent).unwrap();
        assert!(verify(&fixture).is_err(), "{field}");
        fs::write(&intent_path, &original_intent).unwrap();
        fs::write(&receipt_path, &original_receipt).unwrap();
    }
    for name in [
        "identity-publication-extra.json",
        "identity-publication-any.pending",
        "verified-EXTRA.json",
    ] {
        let path = root.join(name);
        fs::write(&path, b"{}").unwrap();
        assert!(verify(&fixture).is_err(), "{name}");
        fs::remove_file(path).unwrap();
    }
    assert!(verify(&fixture).is_ok());
}

#[test]
fn ordered_complete_ledger_lock_and_template_bytes_are_mandatory() {
    let fixture = verified();
    let root = fixture.root();
    let path = root.join("ledger.json");
    let original = fs::read(&path).unwrap();
    for change in 0..5 {
        let mut ledger = evidence::parse(&original).unwrap();
        match change {
            0 => ledger["status"] = json!("needs-reconciliation"),
            1 => ledger["entries"][0]["phase"] = json!("started"),
            2 => {
                ledger["entries"].as_array_mut().unwrap().swap(0, 1);
            }
            3 => {
                ledger["entries"].as_array_mut().unwrap().pop();
            }
            _ => ledger["publication_nonce"] = json!("bad"),
        }
        document::write_replace(&path, &ledger).unwrap();
        assert!(verify(&fixture).is_err(), "{change}");
        fs::write(&path, &original).unwrap();
    }
    fs::create_dir(root.join("lock")).unwrap();
    assert!(verify(&fixture).is_err());
    fs::remove_dir(root.join("lock")).unwrap();
    let value = verify(&fixture).unwrap();
    let template = std::path::Path::new(value["templates"][0]["path"].as_str().unwrap());
    fs::write(template, b"changed template").unwrap();
    assert!(verify(&fixture).is_err());
}
