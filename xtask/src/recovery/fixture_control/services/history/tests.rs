use std::{fs, path::PathBuf};

use super::{
    evidence::{binding, read},
    *,
};
use crate::recovery::clone::native::{digest, json as json_io};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

struct Fixture {
    _temporary: tempfile::TempDir,
    run: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&workspace).unwrap();
        let temporary = tempfile::tempdir_in(&workspace).unwrap();
        let run = super::super::super::portable_path(&fs::canonicalize(temporary.path()).unwrap());
        json_io::write_new(&run.join("manifest.json"), &json!({"fixture":true})).unwrap();
        state::initialize(&run).unwrap();
        let fixture = Self {
            _temporary: temporary,
            run,
        };
        for (stage, mode) in INITIAL {
            let (number, _) = state::begin(&fixture.run, stage, mode, &json!({})).unwrap();
            state::finish(&fixture.run, number, Some(&json!({"service":stage})), None).unwrap();
        }
        fixture
    }
    fn load(&self) -> Value {
        state::load(&self.run, true).unwrap()
    }
    fn history(&self) -> Value {
        validate(&self.run, &self.load(), true, true).unwrap()
    }
    fn stage(&self, mode: &str, proof: Option<&Value>) -> u64 {
        let sources = json!({"manifest":binding(&self.run.join("manifest.json")).unwrap(),"state_before":binding(&self.run.join("state.json")).unwrap()});
        let (number, _) = state::begin(&self.run, STAGE, mode, &sources).unwrap();
        state::finish(
            &self.run,
            number,
            proof,
            proof.is_none().then_some("InjectedFailure"),
        )
        .unwrap();
        number
    }
    fn close(&self) -> Value {
        let root = self.run.join("lifecycle-0004");
        fs::create_dir(&root).unwrap();
        fs::create_dir(self.run.join("rustfs")).unwrap();
        let tree = json!({"format_version":2,"runtime_directory":self.run.join("rustfs"),"role":"rustfs",
            "scope_id":"fixture-test","operation_id":"b".repeat(32),"supervisor":{"pid":100},"monitor":{"pid":101},"process":{"pid":102}});
        let completion = self.run.join(format!(
            "rustfs/rustfs-members-{}-stopped.json",
            "b".repeat(32)
        ));
        json_io::write_new(&completion,&json!({"status":"stopped","error_type":null,"operation_id":tree["operation_id"],
            "role":"rustfs","scope_id":"fixture-test","supervisor":tree["supervisor"],"monitor":tree["monitor"],
            "directory":tree["runtime_directory"],"members":[tree["supervisor"],tree["monitor"],tree["process"]]})).unwrap();
        json_io::write_new(
            &root.join("stopped.json"),
            &json!({"status":"redis_process_stopped","alive":false,"resources_deleted":false}),
        )
        .unwrap();
        json_io::write_new(
            &root.join("rustfs-stopped.json"),
            &json!({"state":"stopped","tree":tree,"completion":binding(&completion).unwrap()}),
        )
        .unwrap();
        json!({"kind":"reference-fixture-service-lifecycle","operation":"close","run":self.run,
            "remote_writes":0,"resources_deleted":false,"status":"services_closed","services":{"redis":"stopped","rustfs":"stopped"},
            "evidence":{"redis":binding(&root.join("stopped.json")).unwrap(),"rustfs":binding(&root.join("rustfs-stopped.json")).unwrap()}})
    }
}

#[test]
fn initial_order_readiness_and_unsettled_are_strict() {
    let fixture = Fixture::new();
    assert_eq!(fixture.history()["initial_complete"], true);
    for index in 0..3 {
        let mut value = fixture.load();
        value["attempts"][index]["stage"] = json!("unknown");
        assert!(validate(&fixture.run, &value, true, true).is_err());
        value = fixture.load();
        value["attempts"][index]["status"] = json!("failed");
        assert!(validate(&fixture.run, &value, true, true).is_err());
    }
    fixture.stage("close", None);
    assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
    let history = validate(&fixture.run, &fixture.load(), true, false).unwrap();
    assert_eq!(history["unsettled"], json!([4]));
    assert!(reconcilable(&fixture.load(), &history, "close").is_some());
    assert!(reconcilable(&fixture.load(), &history, "restart").is_none());
}

#[test]
fn close_preserves_prefix_is_readonly_and_rejects_tampered_members() {
    let fixture = Fixture::new();
    let proof = fixture.close();
    let before = binding(&fixture.run.join("state.json")).unwrap();
    fixture.stage("close", Some(&proof));
    let snapshot = fs::read(fixture.run.join("state.json")).unwrap();
    assert_eq!(fixture.history()["closed"], true);
    assert_eq!(prefix::count(&fixture.run, &before).unwrap(), 3);
    assert_eq!(fs::read(fixture.run.join("state.json")).unwrap(), snapshot);
    let completion = fixture.run.join(format!(
        "rustfs/rustfs-members-{}-stopped.json",
        "b".repeat(32)
    ));
    fs::write(completion, b"tampered").unwrap();
    assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
}

#[test]
fn close_rejects_duplicate_unknown_stage_and_wrong_predecessor() {
    let fixture = Fixture::new();
    let proof = fixture.close();
    fixture.stage("close", Some(&proof));
    let mut value = fixture.load();
    value["attempts"][3]["mode"] = json!("unknown");
    assert!(validate(&fixture.run, &value, true, true).is_err());
    fixture.stage("close", Some(&proof));
    assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
}

#[test]
fn history_rejects_nonzero_writes_resource_deletion_and_forged_run() {
    for (key, value) in [
        ("remote_writes", json!(1)),
        ("resources_deleted", json!(true)),
        ("run", json!("elsewhere")),
    ] {
        let fixture = Fixture::new();
        let mut proof = fixture.close();
        proof[key] = value;
        fixture.stage("close", Some(&proof));
        assert!(validate(&fixture.run, &fixture.load(), true, true).is_err());
    }
}

#[test]
fn prefix_preserves_all_writers_unicode_and_crlf() {
    let fixture = Fixture::new();
    let mut initial = fixture.load();
    initial["attempts"][0]["sources"] = json!({"中文":"🌏\u{7f}"});
    json_io::write_replace(&fixture.run.join("state.json"), &initial).unwrap();
    let raw = serde_json::to_string(&initial).unwrap();
    let descriptors = prefix::serializations(&raw).into_iter().map(|raw| json!({"path":fixture.run.join("state.json"),"bytes":raw.len(),"sha256":hex::encode(Sha256::digest(&raw))})).collect::<Vec<_>>();
    fixture.stage("close", None);
    for descriptor in descriptors {
        assert_eq!(prefix::count(&fixture.run, &descriptor).unwrap(), 3);
    }
    let mut descriptor = binding(&fixture.run.join("state.json")).unwrap();
    descriptor["sha256"] = json!("a".repeat(64));
    assert!(prefix::count(&fixture.run, &descriptor).is_err());
}

#[test]
fn prefix_serialization_matches_python_escaping() {
    let variants = prefix::serializations(r#"{"z":"\u007f中文🌏","a":[]}"#);
    assert_eq!(
        String::from_utf8(variants[0].clone()).unwrap(),
        "{\n  \"a\": [],\n  \"z\": \"\u{7f}中文🌏\"\n}\n"
    );
    assert_eq!(
        String::from_utf8(variants[1].clone()).unwrap(),
        "{\n  \"z\": \"\\u007f\\u4e2d\\u6587\\ud83c\\udf0f\",\n  \"a\": []\n}\n"
    );
}

#[test]
fn protocol_rejects_unknown_duplicate_and_oversized_without_echoing_secrets() {
    let secret = "private-credential-marker";
    let inputs = [
        format!(r#"{{"format_version":1,"operation":"{secret}","backend":"x","arguments":{{}}}}"#)
            .into_bytes(),
        format!(r#"{{"format_version":1,"format_version":2,"password":"{secret}"}}"#).into_bytes(),
        vec![b'X'; 4 * 1024 * 1024 + 1],
    ];
    for input in inputs {
        let (code, response) = protocol::execute(&input);
        assert_eq!(code, 2);
        assert!(!response.to_string().contains(secret));
    }
}

#[test]
fn manifest_rejects_unknown_fields_and_false_empty_directory() {
    let mut manifest = json!({"format_version":1,"kind":"reference-fixture-service-run","review":{},"bootstrap":{},
        "execution_backend":"backend","scope_id":"fixture","data_directory_was_empty":true});
    validate_manifest(&manifest).unwrap();
    manifest["data_directory_was_empty"] = json!(false);
    assert!(validate_manifest(&manifest).is_err());
    manifest["data_directory_was_empty"] = json!(true);
    manifest["extra"] = json!(true);
    assert!(validate_manifest(&manifest).is_err());
}

#[test]
fn reconcilable_restart_requires_only_one_last_failed_restart_and_external_owner() {
    let value = json!({"attempts":[{"number":1,"stage":STAGE,"mode":"restart","status":"failed"}]});
    let history = json!({"closed":false,"external_recovery":{"path":"recovery"},"unsettled":[1]});
    assert!(reconcilable(&value, &history, "restart").is_some());
    for (key, value) in [
        ("closed", json!(true)),
        ("external_recovery", Value::Null),
        ("unsettled", json!([1, 2])),
    ] {
        let mut invalid = history.clone();
        invalid[key] = value;
        assert!(reconcilable(&json!({"attempts":[]}), &invalid, "restart").is_none());
    }
}

#[test]
fn bound_evidence_rejects_path_escape_and_content_drift() {
    let fixture = Fixture::new();
    let mut descriptor = binding(&fixture.run.join("manifest.json")).unwrap();
    descriptor["path"] = json!(fixture.run.join("../outside.json"));
    assert!(evidence::bound(&fixture.run, &descriptor).is_err());
    let descriptor = binding(&fixture.run.join("manifest.json")).unwrap();
    fs::write(fixture.run.join("manifest.json"), b"{}").unwrap();
    assert!(evidence::bound(&fixture.run, &descriptor).is_err());
    assert_eq!(read(&fixture.run.join("manifest.json")).unwrap(), json!({}));
    assert_eq!(
        digest::file(&fixture.run.join("manifest.json"))
            .unwrap()
            .bytes,
        2
    );
}
