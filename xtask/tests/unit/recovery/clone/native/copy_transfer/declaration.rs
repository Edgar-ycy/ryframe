use std::path::PathBuf;

use super::*;

const SOURCE: &str = "fixture-source";
const TARGET: &str = "fixture-target";
const SOURCE_ENDPOINT: &str = "http://127.0.0.1:29601";
const TARGET_ENDPOINT: &str = "http://127.0.0.1:29602";
const UUID: &str = "81fd3774-5440-4ccb-b270-d0f04988e724";

struct Fixture {
    export: Value,
    source: BTreeMap<String, Value>,
    target: Value,
    initialized: BTreeMap<String, Value>,
    evidence: Value,
    execution: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let source = config(SOURCE, "source", [2, 0, 3, 1]);
        let target = config(TARGET, "target", [3, 1, 0, 2]);
        let mut buckets = BUCKETS
            .iter()
            .rev()
            .map(|bucket| json!({"bucket":bucket,"entries":[]}))
            .collect::<Vec<_>>();
        buckets.last_mut().unwrap()["entries"] = json!(["z.bin", "a.bin"].map(|file| {
            json!({"key":format!("{SOURCE}/{file}"),"artifact":artifact_binding(file),
                "metadata":object_metadata(),"capture":{"private_example":"discard"},
                "get_header_consistent":true})
        }));
        let dumps = source["databases"].as_array().unwrap().iter().map(|db| {
            let key = db["key"].as_str().unwrap();
            json!({"key":key,"tables":{"biz_example":0},"artifact":artifact_binding(&format!("{key}.sql"))})
        }).collect::<Vec<_>>();
        let schedule = ["30", "1", "9"].map(|id| {
            json!({
                "tenant_id":"system","schedule_id":id,"handler_key":"sample.jobs",
                "expected_version":7,"source_row_sha256":"b".repeat(64)
            })
        });
        let fixture_root = crate::workspace::root_dir().join("absent-declaration-fixture");
        let evidence = json!({
            "source_stopped":artifact_binding("generation-after.json"),
            "target_initialized":artifact_binding("proof/initialized.json"),
            "target_stopped":artifact_binding("proof/stopped.json")
        });
        let export = json!({
            "scope_id":SOURCE,"source":source,"artifact_root":fixture_root.join("artifacts"),
            "worktree_fingerprint":format!("sha256:{}", "c".repeat(64)),
            "source_snapshot":{"head":"d".repeat(40),"clean":false,"other":"discard"},
            "databases":dumps,"objects":buckets,"enabled_system_schedule_rows":schedule,
            "evidence":{"generation-after":evidence["source_stopped"]},
        });
        Self {
            source: observations(&source),
            initialized: observations(&target),
            target,
            export,
            evidence,
            execution: fixture_root.join("backend"),
        }
    }

    fn request(&self, stage: &'static str) -> Declaration<'_> {
        Declaration {
            export: &self.export,
            source: &self.source,
            target: &self.target,
            target_endpoint: TARGET_ENDPOINT,
            initialized: &self.initialized,
            evidence: &self.evidence,
            copy_id: "fixture-copy",
            stage,
            execution_backend: &self.execution,
        }
    }
}

fn config(scope: &str, prefix: &str, order: [usize; 4]) -> Value {
    let databases = order.map(|index| {
        let (key, kind, mode) = domain::TARGET_DATABASES[index];
        json!({"key":key,"kind":kind,"mode":mode,"server_uuid":UUID,
            "database":format!("{prefix}_{index}"),"defaults_file":"private-fixture-defaults"})
    });
    json!({"scope_id":scope,"s3":{"endpoint":SOURCE_ENDPOINT,"region":"fixture"},"databases":databases})
}

fn observations(config: &Value) -> BTreeMap<String, Value> {
    let scope = config["scope_id"].as_str().unwrap();
    config["databases"].as_array().unwrap().iter().map(|db| {
        (db["key"].as_str().unwrap().to_owned(), json!({
            "resource":{"kind":"database","scope_id":scope,
                "server_uuid":db["server_uuid"],"database":db["database"]},
            "schema_sha256":"e".repeat(64),"ownership":expected_owners(scope, db["kind"] == "combined"),
            "tables":{},"preserved":{},"all_tables":[],
        }))
    }).collect()
}

fn artifact_binding(file: &str) -> Value {
    json!({"file":file,"bytes":12,"sha256":"a".repeat(64)})
}

fn object_metadata() -> Value {
    json!({"ContentType":"text/plain","CacheControl":null,"ContentDisposition":null,
        "ContentEncoding":null,"ContentLanguage":null,"Expires":null,
        "WebsiteRedirectLocation":null,"Metadata":{}})
}

fn row_keys(value: &Value, field: &str) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row[field].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn declaration_matches_existing_fields_order_and_original_digests() {
    let fixture = Fixture::new();
    let value = compose(fixture.request("source_to_seed")).unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "app_env",
            "artifact_root",
            "copy_id",
            "copy_stage",
            "databases",
            "evidence",
            "execution_backend",
            "format_version",
            "objects",
            "source",
            "source_snapshot",
            "target",
            "target_schedule_actions"
        ])
    );
    assert_eq!(value["app_env"], "test");
    assert_eq!(
        value["execution_backend"],
        fixture.execution.to_str().unwrap()
    );
    assert_eq!(
        value["source_snapshot"],
        json!({"head":"d".repeat(40),"clean":false,"worktree_sha256":"c".repeat(64)})
    );
    assert_eq!(value["databases"], fixture.export["databases"]);
    assert_eq!(value["evidence"], fixture.evidence);
    assert_eq!(value["source"]["object_endpoint"], SOURCE_ENDPOINT);
    assert_eq!(value["target"]["object_endpoint"], TARGET_ENDPOINT);
    assert_eq!(value["target"]["state"], "new_initialized");
    assert_eq!(value["target"]["ever_started"], false);
    assert_eq!(
        row_keys(&value["source"]["databases"], "key"),
        ["dedicated-a", "shared-control", "dedicated-b", "shared"]
    );
    assert_eq!(
        row_keys(&value["target"]["databases"], "key"),
        ["dedicated-b", "shared", "shared-control", "dedicated-a"]
    );
    assert_eq!(
        value["target"]["databases"][2]["ownership"],
        fixture.initialized["shared-control"]["ownership"]
    );
    assert_eq!(
        row_keys(&value["objects"], "bucket"),
        ["config-packages", "imports", "exports", "avatar", "uploads"]
    );
    let objects = &value["objects"][4]["entries"];
    assert_eq!(
        row_keys(objects, "key"),
        ["fixture-source/z.bin", "fixture-source/a.bin"]
    );
    assert_eq!(
        objects[0],
        json!({"key":"fixture-source/z.bin","artifact":artifact_binding("z.bin"),
        "metadata":object_metadata()})
    );
    assert_eq!(
        row_keys(&value["target_schedule_actions"], "schedule_id"),
        ["30", "1", "9"]
    );
    assert_eq!(
        value["target_schedule_actions"][0]["action"],
        "disable_schedule_via_api"
    );
    assert_eq!(
        value["target_schedule_actions"][0]["source_row_sha256"],
        "b".repeat(64)
    );
    assert!(!value.to_string().contains("private-fixture-defaults"));
}

#[test]
fn both_database_sides_must_match_all_registered_physical_identities_and_owners() {
    let mut fixture = Fixture::new();
    fixture.source.remove("shared");
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    fixture.initialized.get_mut("shared").unwrap()["resource"]["scope_id"] = json!(SOURCE);
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    fixture.initialized.get_mut("shared").unwrap()["ownership"] =
        json!(expected_owners(SOURCE, false));
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    fixture.initialized.get_mut("shared").unwrap()["schema_sha256"] = json!("f".repeat(64));
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    fixture.target["databases"][1]["database"] = json!("source_1");
    fixture.initialized.get_mut("shared").unwrap()["resource"]["database"] = json!("source_1");
    assert!(compose(fixture.request("source_to_seed")).is_err());
}

#[test]
fn original_source_proof_fingerprint_and_execution_path_stay_bound() {
    let mut fixture = Fixture::new();
    fixture.evidence["source_stopped"] = artifact_binding("wrong-generation.json");
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    fixture.export["worktree_fingerprint"] = json!(format!("sha256:{}", "G".repeat(64)));
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    fixture.execution = fixture.execution.join("../other");
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    fixture.export["scope_id"] = json!(TARGET);
    assert!(compose(fixture.request("source_to_seed")).is_err());
}

#[test]
fn object_and_schedule_projection_keep_the_offline_constraints() {
    let mut fixture = Fixture::new();
    fixture.export["objects"][4]["entries"][0]["key"] = json!(format!("{TARGET}/z.bin"));
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    fixture.export["objects"].as_array_mut().unwrap().pop();
    assert!(compose(fixture.request("source_to_seed")).is_err());
    let mut fixture = Fixture::new();
    assert!(compose(fixture.request("seed_to_arm")).is_err());
    fixture.export["enabled_system_schedule_rows"] = json!([]);
    assert!(compose(fixture.request("seed_to_arm")).is_ok());
    let mut fixture = Fixture::new();
    fixture.export["enabled_system_schedule_rows"][1]["expected_version"] = json!(0);
    assert!(compose(fixture.request("source_to_seed")).is_err());
}
