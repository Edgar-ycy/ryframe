use super::{evidence, execution, seed_evidence, test_fixture::Fixture};
use crate::recovery::clone::native::{
    digest, json as document, lifecycle::state, resources::common, seed,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

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
    for path in [&intent_path, &receipt_path] {
        let original = fs::read(path).unwrap();
        fs::remove_file(path).unwrap();
        assert!(verify(&fixture).is_err(), "缺少 {}", path.display());
        fs::write(path, &original).unwrap();
    }
    fs::write(&receipt_path, b"{\"replacement\":true}\n").unwrap();
    assert!(verify(&fixture).is_err());
    fs::write(&receipt_path, &original_receipt).unwrap();
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
        reject_resigned_receipt(&fixture, receipt);
    }
    let pending = intent_path.with_extension("json.pending");
    fs::write(&pending, &original_intent).unwrap();
    assert!(verify(&fixture).is_err());
    fs::remove_file(pending).unwrap();
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
    for case in 0..7 {
        let mut receipt = evidence::parse(&original_receipt).unwrap();
        match case {
            0 => receipt["identities"].as_array_mut().unwrap().reverse(),
            1 => receipt["identity_pools"] = json!({}),
            2 => {
                receipt["message_audience"] = json!([]);
                receipt["audience_sha256"] = json!(digest::plan_hash(&json!([])).unwrap());
            }
            3 => receipt["homepage_user_slots"] = json!([]),
            4 => receipt["templates"][0]["department_sha256"] = json!("0".repeat(64)),
            5 => receipt["identities"][0]["user_id"] = json!("999999"),
            _ => receipt["templates"][0]["tenant_slot"] = json!("tenant-02"),
        }
        let error = reject_resigned_receipt(&fixture, receipt);
        assert!(
            error.contains("收据") || error.contains("模板"),
            "{case}: {error}"
        );
    }
    assert!(verify(&fixture).is_ok());
}

fn reject_resigned_receipt(fixture: &Fixture, receipt: Value) -> String {
    let root = fixture.root();
    let (intent, verified) =
        seed_evidence::names(fixture.plan["plan_sha256"].as_str().unwrap(), &root);
    let intent_path = root.join(intent);
    let receipt_path = root.join(verified);
    let original_intent = fs::read(&intent_path).unwrap();
    let original_receipt = fs::read(&receipt_path).unwrap();
    document::write_replace(&receipt_path, &receipt).unwrap();
    let mut intent = evidence::parse(&original_intent).unwrap();
    intent["receipt"] = receipt;
    intent["receipt_sha256"] = json!(digest::file(&receipt_path).unwrap().sha256);
    document::write_replace(&intent_path, &intent).unwrap();
    let error = verify(fixture).unwrap_err().to_string();
    fs::write(&intent_path, original_intent).unwrap();
    fs::write(&receipt_path, original_receipt).unwrap();
    error
}

#[test]
fn ordered_complete_ledger_lock_and_template_bytes_are_mandatory() {
    let fixture = verified();
    let root = fixture.root();
    let path = root.join("ledger.json");
    let original = fs::read(&path).unwrap();
    for change in 0..9 {
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
            4 => ledger["publication_nonce"] = json!("bad"),
            5 => {
                ledger.as_object_mut().unwrap().remove("publication_nonce");
            }
            6 => ledger["entries"] = json!([]),
            7 => ledger["verified_receipt"] = json!("../verified.json"),
            _ => ledger["plan_sha256"] = json!("a".repeat(64)),
        }
        document::write_replace(&path, &ledger).unwrap();
        assert!(verify(&fixture).is_err(), "{change}");
        fs::write(&path, &original).unwrap();
    }
    fs::create_dir(root.join("lock")).unwrap();
    assert!(verify(&fixture).is_err());
    fs::remove_dir(root.join("lock")).unwrap();
    fs::write(root.join("lock"), b"unreleased").unwrap();
    assert!(verify(&fixture).is_err());
    fs::remove_file(root.join("lock")).unwrap();
    let value = verify(&fixture).unwrap();
    let template = std::path::Path::new(value["templates"][0]["path"].as_str().unwrap());
    fs::write(template, b"changed template").unwrap();
    assert!(verify(&fixture).is_err());
}

#[test]
fn publication_cannot_be_copied_to_a_different_identity_root() {
    let fixture = verified();
    let copied = fixture.directory.join("copied-identities");
    copy_directory(&fixture.root(), &copied);
    assert!(seed_evidence::verify(&crate::workspace::root_dir(), &fixture.plan, &copied).is_err());
    assert!(verify(&fixture).is_ok());
}

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for item in fs::read_dir(source).unwrap() {
        let item = item.unwrap();
        let kind = item.file_type().unwrap();
        assert!(!kind.is_symlink());
        let target = destination.join(item.file_name());
        if kind.is_dir() {
            copy_directory(&item.path(), &target);
        } else {
            assert!(kind.is_file());
            fs::copy(item.path(), target).unwrap();
        }
    }
}

fn registration(fixture: &Fixture, run: &Path) -> Value {
    fs::create_dir(run).unwrap();
    document::write_new(&run.join("manifest.json"), &json!({"fixture":true})).unwrap();
    state::initialize(run).unwrap();
    let plan = run.join("plan.json");
    document::write_new(&plan, &fixture.plan).unwrap();
    let request = json!({"format_version":1,"kind":"devex-clone-seed-runtime",
        "run_manifest":common::binding(&run.join("manifest.json")).unwrap(),
        "post_copy":{},"post_verify":{},"identity_plan":common::binding(&plan).unwrap(),
        "identity_state":fixture.root(),"identity_tool":{},"api_process":{},"node":{}});
    document::write_new(&run.join("seed-runtime.json"), &request).unwrap();
    let (number, _) = state::begin(run, "seed-runtime", "register", &json!({})).unwrap();
    state::finish(
        run,
        number,
        Some(&json!({"status":"seed_runtime_registered",
        "registration":common::binding(&run.join("seed-runtime.json")).unwrap()})),
        None,
    )
    .unwrap();
    request
}

#[test]
fn identity_protocol_requires_the_exact_registered_request_and_current_stage_without_writes() {
    let fixture = verified();
    let backend = crate::workspace::root_dir();
    let run = fixture.directory.join("seed-run");
    let request = registration(&fixture, &run);
    let invoke = |arguments| {
        seed::protocol::execute(
            &backend,
            &serde_json::to_vec(&json!({
        "format_version":1,"kind":"ryframe-seed-evidence","run_dir":run,
        "operation":"identity-evidence","arguments":arguments}))
            .unwrap(),
        )
    };
    let state_before = fs::read(run.join("state.json")).unwrap();
    let ledger_before = fs::read(fixture.root().join("ledger.json")).unwrap();
    assert_eq!(
        invoke(json!({"request":request})).unwrap(),
        verify(&fixture).unwrap()
    );
    for arguments in [json!({}), json!({"request":request,"extra":false})] {
        assert!(invoke(arguments).is_err());
    }
    let mut other = request.clone();
    other["identity_state"] = json!(fixture.directory.join("copied-identities"));
    for value in [other, Value::Null] {
        let error = invoke(json!({"request":value})).unwrap_err().to_string();
        assert!(error.contains("请求不同于当前已登记请求"), "{error}");
    }
    assert_eq!(state_before, fs::read(run.join("state.json")).unwrap());
    assert_eq!(
        ledger_before,
        fs::read(fixture.root().join("ledger.json")).unwrap()
    );
    assert!(!run.join("run.lock").exists());
    let (number, _) = state::begin(&run, "seed-runtime", "register", &json!({})).unwrap();
    assert!(invoke(json!({"request":request})).is_err());
    state::finish(&run, number, None, Some("fixture-failure")).unwrap();
    assert!(invoke(json!({"request":request})).is_err());
}
