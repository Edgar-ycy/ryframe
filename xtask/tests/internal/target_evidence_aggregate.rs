use crate::{
    Result,
    recovery::clone::native::{
        digest, json as json_io,
        target_verify::{
            evidence_creation, evidence_prepare, evidence_protocol, evidence_reset,
            input::request::KEYS,
        },
    },
    workspace,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::PathBuf};

struct Fixture {
    backend: PathBuf,
    _directory: tempfile::TempDir,
    target: PathBuf,
    request: Value,
    descriptor: Value,
}
impl Fixture {
    fn new() -> Self {
        let backend = workspace::root_dir();
        let parent = backend.join(".local-tests/target-evidence-aggregate");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        let target = directory.path().join("target");
        fs::create_dir(&target).unwrap();
        let defaults = directory.path().join("mysql.cnf");
        fs::write(&defaults,"[client]\nhost=127.0.0.1\nport=3306\nuser=fixture\npassword=private-secret\nssl-mode=DISABLED\n").unwrap();
        let request = json!({"target":{"scope_id":"fixture-scope","databases":KEYS.map(|(key,_,_)|json!({"key":key,"server_uuid":"fixture-uuid","database":key.replace('-',"_"),"defaults_file":defaults,"defaults_sha256":digest::file(&defaults).unwrap().sha256}))},
            "storage":{"redis":{"port":16379}},"reset":{"sentinel_key":"fixture-sentinel","sentinel_value":"fixture-value","credential_version":"fixture-v1","legacy_ownership":{"mysql_exclusive":true,"redis_exclusive":true,"object_storage_exclusive":true}}});
        let mut fixture = Self {
            backend,
            _directory: directory,
            target,
            request,
            descriptor: Value::Null,
        };
        fixture.put("request.json", &fixture.request);
        fixture.put("prepare.json", &json!({"status":"prepared"}));
        fixture.descriptor = fixture.binding("request.json");
        fixture
    }
    fn put(&self, name: &str, value: &Value) {
        fs::write(self.target.join(name), serde_json::to_vec(value).unwrap()).unwrap();
    }
    fn binding(&self, name: &str) -> Value {
        let path = self.target.join(name);
        let actual = digest::file(&path).unwrap();
        json!({"path":path,"bytes":actual.bytes,"sha256":actual.sha256})
    }
    fn stage(&self, stage: &str, kind: &str, value: &Value) {
        let field = if kind == "intent" {
            "resource"
        } else {
            "observed"
        };
        let mut document = json!({"at":"2026-10-23T00:00:01Z","stage":stage});
        document[field] = value.clone();
        self.put(&format!("{stage}.{kind}.json"), &document);
    }
    fn creation(&self) -> Result<Value> {
        for db in self.request["target"]["databases"].as_array().unwrap() {
            let stage = format!("create-{}", db["key"].as_str().unwrap());
            self.stage(
                &stage,
                "intent",
                &json!({"key":db["key"],"server_uuid":db["server_uuid"],"database":db["database"]}),
            );
            self.stage(&stage,"confirmed",&json!({"server_uuid":db["server_uuid"],"database":db["database"],"exists":true,"empty_checked":true}));
        }
        self.stage(
            "sentinel",
            "intent",
            &json!({"key":self.request["reset"]["sentinel_key"]}),
        );
        self.stage(
            "sentinel",
            "confirmed",
            &json!({"keys":[],"owner":null,"sentinel":self.request["reset"]["sentinel_value"]}),
        );
        evidence_creation::creation(&self.backend, &self.target, &self.request)
    }
    fn prepare_failure(&self) {
        self.put("failure.json",&json!({"status":"needs_reconciliation","stage":"prepare_absence","at":"2026-10-23T00:00:00Z","error_type":"ValueError","reason":"fixture","automatic_retry":false,"automatic_resource_cleanup":false,"fresh_target_initialized":false,"clone_verified":false,"restore_qualified":false}));
    }
    fn resume(&self) -> (String, String) {
        self.prepare_failure();
        let intent = format!("resume-prepare-{}.intent.json", "a".repeat(32));
        let confirmed = format!("resume-prepare-{}.confirmed.json", "a".repeat(32));
        self.put(&intent,&json!({"format_version":1,"kind":"devex-clone-prepare-resume-intent","attempt":"a".repeat(32),"at":"2026-10-23T00:00:01Z","request":self.descriptor,
            "files_before":{"request.json":self.binding("request.json"),"failure.json":self.binding("failure.json")}}));
        self.put(&confirmed,&json!({"format_version":1,"kind":"devex-clone-prepare-resume-confirmed","attempt":"a".repeat(32),"at":"2026-10-23T08:00:02+08:00","intent":self.binding(&intent),"remote_writes":0,"prepare":self.binding("prepare.json"),"request":self.binding("request.json")}));
        self.put(
            "initialize.started.json",
            &json!({"at":"2026-10-23T00:00:03Z","generation_sha256":"b".repeat(64)}),
        );
        (intent, confirmed)
    }
    fn mutate(&self, name: &str, change: impl FnOnce(&mut Value)) {
        let mut value = json_io::read(&self.target.join(name)).unwrap();
        change(&mut value);
        self.put(name, &value);
    }
}

#[test]
fn creation_binds_every_stage_and_rejects_extra_missing_or_changed_evidence() {
    let fixture = Fixture::new();
    let result = fixture.creation().unwrap();
    assert_eq!(result.as_object().unwrap().len(), 5);
    assert_eq!(
        result["sentinel"]["intent"],
        fixture.binding("sentinel.intent.json")
    );
    let before = fs::read(fixture.target.join("sentinel.confirmed.json")).unwrap();
    assert_eq!(
        evidence_creation::creation(&fixture.backend, &fixture.target, &fixture.request).unwrap(),
        result
    );
    assert_eq!(
        fs::read(fixture.target.join("sentinel.confirmed.json")).unwrap(),
        before
    );
    fixture.put("create-foreign.intent.json", &json!({}));
    assert!(
        evidence_creation::creation(&fixture.backend, &fixture.target, &fixture.request).is_err()
    );
    fs::remove_file(fixture.target.join("create-foreign.intent.json")).unwrap();
    fixture.mutate("sentinel.confirmed.json", |value| {
        value["observed"]["sentinel"] = json!("other")
    });
    assert!(
        evidence_creation::creation(&fixture.backend, &fixture.target, &fixture.request).is_err()
    );
    fs::remove_file(fixture.target.join("sentinel.confirmed.json")).unwrap();
    assert!(
        evidence_creation::creation(&fixture.backend, &fixture.target, &fixture.request).is_err()
    );
}

#[test]
fn prepare_history_binds_preimages_offsets_and_initialization_order() {
    let fixture = Fixture::new();
    let (intent, confirmed) = fixture.resume();
    let stages =
        evidence_prepare::stages(&fixture.backend, &fixture.target, &fixture.descriptor).unwrap();
    assert_eq!(stages, BTreeSet::from([intent.clone(), confirmed]));
    let records =
        evidence_prepare::records(&fixture.backend, &fixture.target, &fixture.descriptor).unwrap();
    assert_eq!(records["pending"], json!([]));
    assert_eq!(records["intents"]["a".repeat(32)], fixture.binding(&intent));
    fixture.mutate("request.json", |value| value["changed"] = json!(true));
    assert!(
        evidence_prepare::records(&fixture.backend, &fixture.target, &fixture.descriptor)
            .unwrap_err()
            .to_string()
            .contains("前像变化")
    );
}

#[test]
fn prepare_history_refuses_orphans_duplicates_pending_unknown_files_and_causal_drift() {
    for case in 0..9 {
        let fixture = Fixture::new();
        let (intent, confirmed) = fixture.resume();
        match case {
            0 => fs::remove_file(fixture.target.join(&intent)).unwrap(),
            1 => {
                let failure = confirmed.replace(".confirmed.", ".failure.");
                fixture.put(&failure,&json!({"format_version":1,"kind":"devex-clone-prepare-resume-failure","attempt":"a".repeat(32),"at":"2026-10-23T00:00:02Z","intent":fixture.binding(&intent),"remote_writes":0,"error_type":"ValueError","reason":"fixture"}));
            }
            2 => fs::remove_file(fixture.target.join(&confirmed)).unwrap(),
            3 => fixture.put("resume-prepare-orphan.json", &json!({})),
            4 => fixture.mutate(&confirmed, |value| {
                value["at"] = json!("2026-10-23T00:00:04Z")
            }),
            5 => fixture.mutate(&confirmed, |value| {
                value["intent"]["sha256"] = json!("0".repeat(64))
            }),
            6 => fixture.mutate(&confirmed, |value| {
                value["prepare"]["sha256"] = json!("0".repeat(64))
            }),
            7 => fixture.mutate(&confirmed, |value| value["remote_writes"] = json!(1)),
            _ => fixture.mutate(&intent, |value| value["at"] = json!("2026-10-23T00:00:04Z")),
        }
        assert!(
            evidence_prepare::stages(&fixture.backend, &fixture.target, &fixture.descriptor)
                .is_err(),
            "case {case}"
        );
    }
}

#[test]
fn initialization_failure_requires_a_closed_prepare_predecessor() {
    let fixture = Fixture::new();
    let (_, confirmed) = fixture.resume();
    let secondary = format!("failure-{}.json", "b".repeat(32));
    fixture.put(&secondary, &json!({"stage":"create_and_reset"}));
    assert_eq!(
        evidence_prepare::failure_path(&fixture.backend, &fixture.target, &fixture.descriptor)
            .unwrap(),
        fixture.binding(&secondary)
    );
    fs::remove_file(fixture.target.join(confirmed)).unwrap();
    assert!(
        evidence_prepare::failure_path(&fixture.backend, &fixture.target, &fixture.descriptor)
            .is_err()
    );
    fixture.put("failure-invalid.json", &json!({}));
    assert!(
        evidence_prepare::failure_path(&fixture.backend, &fixture.target, &fixture.descriptor)
            .is_err()
    );
}

#[test]
fn initialization_paths_preserve_previous_ledger_and_refuse_outside_descriptors() {
    let fixture = Fixture::new();
    let creation = fixture.creation().unwrap();
    let descriptor = fixture.binding("request.json");
    let mut reset = json!({"plan":descriptor,"plan_confirm":descriptor,"intent":descriptor,"confirmed":descriptor,"commands":{},
        "completed":{"report":{"file":"reset-state/report.json"},"ledger":{"file":"reset-state/ledger.json"}}});
    let paths = evidence_creation::paths(&fixture.target, &creation, &reset, &json!([])).unwrap();
    assert!(paths.contains("reset-state/.ledger.json.previous"));
    assert!(paths.contains("initialize.started.json"));
    reset["plan"]["path"] = json!(fixture.target.parent().unwrap().join("outside"));
    assert!(evidence_creation::paths(&fixture.target, &creation, &reset, &json!([])).is_err());
    let expected = evidence_creation::initial_stages(&fixture.request).unwrap();
    assert_eq!(expected.len(), 12);
    assert!(evidence_creation::stage_names(&fixture.target, &expected).is_err());
}

#[test]
fn protocol_executes_native_prepare_aggregation_without_raw_values() {
    let fixture = Fixture::new();
    let (intent, _) = fixture.resume();
    let request = json!({"format_version":1,"operation":"prepare-resume-stages","backend":fixture.backend,"target":fixture.target,"request_descriptor":fixture.descriptor});
    let result = evidence_protocol::execute(&serde_json::to_vec(&request).unwrap()).unwrap();
    assert!(result["names"].as_array().unwrap().contains(&json!(intent)));
    assert!(!result.to_string().contains("private-secret"));
}

#[test]
fn reset_manifest_binds_four_databases_five_buckets_owner_tls_and_sentinel() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let scope = "fixture-scope";
    let physical=fixture.request["target"]["databases"].as_array().unwrap().iter().map(|db|json!({"key":db["key"],"host":"127.0.0.1","port":3306,"database":db["database"],"tls_mode":"disabled"})).collect::<Vec<_>>();
    let databases=physical.iter().map(|db| {
        let key=db["key"].as_str().unwrap();
        let mut owners=serde_json::Map::new();
        if key=="shared-control" {owners.insert("control".into(),json!(format!("ryframe-owner:v1:{scope}:control")));}
        owners.insert("tenant-data".into(),json!(format!("ryframe-owner:v1:{scope}:tenant-data")));
        json!({"target_keys":[key],"host":db["host"],"port":db["port"],"database":db["database"],"control_baseline":key=="shared-control","tenant_baseline":true,"ownership_markers":owners,
            "connection":{"tls_mode":"disabled","username":"fixture","password_config_key":if key=="shared-control" {"database.primary.password".to_owned()} else {format!("tenant_data.targets.{key}.password")},"tls_ca_sha256":null,"tls_client_cert_sha256":null,"tls_client_key_ref_sha256":null}})
    }).collect::<Vec<_>>();
    let s3 = json!({"backend":"rustfs","endpoint":"http://127.0.0.1:29000","region":"fixture"});
    let redis = json!({"namespace":"fixture:","ownership_key":"fixture:owner","ownership_value":"fixture-owner"});
    let original = json!({"maintenance":{"source":{"snapshot":{"head":"a".repeat(40)}}},"physical":{"databases":physical,"s3":s3},"selected":{"redis":redis}});
    let prefixes=["uploads","avatar","exports","imports","config-packages"].map(|bucket|json!({"bucket":bucket,"prefix":format!("{scope}/"),"ownership_marker_key":format!("{scope}/.ryframe-owner"),"ownership_marker":format!("ryframe-owner:v1:{scope}:object-storage:{bucket}")}));
    let manifest = json!({"manifest_version":5,"environment":"test","scope_id":scope,"code_sha":"a".repeat(40),"config_sha":"b".repeat(64),"credential_version":"fixture-v1","confirmation_phrase":format!("RESET-RYFRAME-test-{scope}"),"legacy_ownership":fixture.request["reset"]["legacy_ownership"],"databases":databases,
        "object_storage":{"backend":s3["backend"],"endpoint":s3["endpoint"],"region":s3["region"],"use_ssl":false,"access_key_config_key":"object_storage.access_key","secret_key_config_key":"object_storage.secret_key","prefixes":prefixes},
        "redis":{"host":"127.0.0.1","port":16379,"database":0,"tls":false,"password_config_key":"redis.password","tls_ca_sha256":null,"tls_client_cert_sha256":null,"tls_client_key_ref_sha256":null,"namespace":redis["namespace"],"ownership_marker_key":redis["ownership_key"],"ownership_marker":redis["ownership_value"],"outside_sentinel_key_sha256":hex::encode(Sha256::digest(b"fixture-sentinel"))}});
    evidence_reset::validate_manifest(&fixture.backend, &manifest, &fixture.request, &original)
        .unwrap();
    for case in 0..16 {
        let mut changed = manifest.clone();
        match case {
            0 => {
                changed["databases"][1]["target_keys"] =
                    changed["databases"][0]["target_keys"].clone()
            }
            1 => changed["databases"][0]["connection"]["username"] = json!("other"),
            2 => changed["databases"][0]["connection"]["tls_ca_sha256"] = json!("c".repeat(64)),
            3 => changed["object_storage"]["prefixes"][0]["prefix"] = json!("outside/"),
            4 => changed["redis"]["outside_sentinel_key_sha256"] = json!("c".repeat(64)),
            5 => changed["databases"][0]["tenant_baseline"] = json!(false),
            6 => {
                changed["object_storage"]["prefixes"][1] =
                    changed["object_storage"]["prefixes"][0].clone()
            }
            7 => changed["code_sha"] = json!("c".repeat(40)),
            8 => changed["manifest_version"] = json!(4),
            9 => changed["manifest_version"] = json!(6),
            10 => {
                changed["databases"][0]["connection"]["password_config_key"] =
                    json!("other.password")
            }
            11 => changed["object_storage"]["secret_key_config_key"] = json!("other.secret_key"),
            12 => changed["redis"]["password_config_key"] = Value::Null,
            13 => {
                changed["redis"]
                    .as_object_mut()
                    .unwrap()
                    .remove("tls_ca_sha256");
            }
            14 => changed["databases"][0]["unexpected"] = json!(true),
            _ => changed["object_storage"]["unexpected"] = json!(true),
        }
        let result = evidence_reset::validate_manifest(
            &fixture.backend,
            &changed,
            &fixture.request,
            &original,
        );
        assert!(result.is_err(), "case {case}");
        if matches!(case, 8 | 9) {
            let message = result.unwrap_err().to_string();
            assert!(
                message.contains("manifest_version")
                    && message.contains('5')
                    && message.contains("status")
                    && message.contains("不自动重放"),
                "{message}"
            );
        }
    }
}
