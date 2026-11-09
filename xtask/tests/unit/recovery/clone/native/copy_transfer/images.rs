use super::*;

const SOURCE: &str = "fixture-old";
const TARGET: &str = "fixture-new";
const ENDPOINT: &str = "http://127.0.0.1:29600";
const SERVER: &str = "81fd3774-5440-4ccb-b270-d0f04988e724";

struct Databases {
    generation: String,
    item: Value,
    source_declared: Value,
    target_declared: Value,
    source: Value,
    target: Value,
    kept: BTreeSet<String>,
}

fn declaration(scope: &str, database: &str) -> Value {
    json!({
        "key":"shared-control","kind":"combined","mode":"shared","server_uuid":SERVER,
        "database":database,"schema_sha256":"a".repeat(64),"ownership":expected_owners(scope,true),
    })
}

fn database_observation(
    declared: &Value,
    scope: &str,
    rows: u64,
    data: &str,
    preserved: &str,
) -> Value {
    json!({
        "resource":{"kind":"database","scope_id":scope,"server_uuid":SERVER,"database":declared["database"]},
        "schema_sha256":declared["schema_sha256"],
        "tables":{"sys_post":{"rows":rows,"sha256":data.repeat(64)}},
        "preserved":{
            "ryframe_resource_ownership":{"rows":2,"sha256":preserved.repeat(64)},
            "seaql_tenant_data_migrations":{"rows":5,"sha256":"1".repeat(64)},
        },
        "all_tables":["sys_post","ryframe_resource_ownership","seaql_tenant_data_migrations"],
        "ownership":declared["ownership"],
    })
}

impl Databases {
    fn new() -> Self {
        let source_declared = declaration(SOURCE, "seed_control");
        let target_declared = declaration(TARGET, "target_control");
        let source = database_observation(&source_declared, SOURCE, 1, "b", "d");
        let target = database_observation(&target_declared, TARGET, 1, "c", "e");
        Self {
            generation: "f".repeat(64),
            item: json!({
                "key":"shared-control","source_database":"seed_control","target_database":"target_control",
                "tables":{"sys_post":1},"artifact":{"file":"control.sql","bytes":12,"sha256":"9".repeat(64)},
                "preserve_target_ownership":true,
            }),
            source_declared,
            target_declared,
            source,
            target,
            kept: ["ryframe_resource_ownership", "seaql_tenant_data_migrations"]
                .map(str::to_owned)
                .into(),
        }
    }

    fn target_side(&self) -> DatabaseSide<'_> {
        DatabaseSide {
            scope: TARGET,
            declaration: &self.target_declared,
            preserved_tables: &self.kept,
        }
    }

    fn images(&self, artifact: &str) -> Result<(Value, Value)> {
        database_pair(
            &self.generation,
            &self.item,
            DatabaseSide {
                scope: SOURCE,
                declaration: &self.source_declared,
                preserved_tables: &self.kept,
            },
            self.target_side(),
            DatabaseBasis {
                source: &self.source,
                initialized: &self.target,
                artifact_sha256: artifact,
            },
        )
    }
}

#[test]
fn database_after_uses_complete_source_rows_and_retains_target_identity_schema_and_preserved() {
    let mut fixture = Databases::new();
    fixture.target["ownership"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let artifact = "9".repeat(64);
    let (before, after) = fixture.images(&artifact).unwrap();
    assert_eq!(
        before,
        database_observed(
            &fixture.generation,
            &fixture.item,
            fixture.target_side(),
            &fixture.target
        )
        .unwrap()
    );
    assert_eq!(
        before["preserved_sha256"],
        digest::plan_hash(&fixture.target["preserved"]).unwrap()
    );
    assert_eq!(
        after["tables_sha256"],
        digest::plan_hash(&fixture.source["tables"]).unwrap()
    );
    let mut only_tables = before.clone();
    only_tables["tables_sha256"] = after["tables_sha256"].clone();
    assert_eq!(only_tables, after);
    fixture.source["tables"]["sys_post"]["sha256"] = json!("6".repeat(64));
    let (same_before, changed_after) = fixture.images(&artifact).unwrap();
    assert_eq!(same_before, before);
    assert_ne!(changed_after["tables_sha256"], after["tables_sha256"]);
    fixture.target["preserved"]["seaql_tenant_data_migrations"]["sha256"] = json!("7".repeat(64));
    let (changed_before, changed_after) = fixture.images(&artifact).unwrap();
    assert_ne!(
        changed_before["preserved_sha256"],
        before["preserved_sha256"]
    );
    assert_eq!(
        changed_after["preserved_sha256"],
        changed_before["preserved_sha256"]
    );
    fixture.target["tables"] = fixture.source["tables"].clone();
    let (before, after) = fixture.images(&artifact).unwrap();
    assert_eq!(before, after);
}

#[test]
fn database_rejects_drifted_owner_row_count_schema_artifact_and_incomplete_tables() {
    let artifact = "9".repeat(64);
    let mut fixture = Databases::new();
    assert!(fixture.images(&"8".repeat(64)).is_err());
    fixture.source["tables"]["sys_post"]["rows"] = json!(2);
    assert!(fixture.images(&artifact).is_err());
    for (field, value) in [
        ("ownership", json!(expected_owners(SOURCE, true))),
        ("schema_sha256", json!("2".repeat(64))),
        (
            "all_tables",
            json!([
                "sys_post",
                "ryframe_resource_ownership",
                "seaql_tenant_data_migrations",
                "sys_post"
            ]),
        ),
    ] {
        let mut fixture = Databases::new();
        fixture.target[field] = value;
        assert!(fixture.images(&artifact).is_err(), "应拒绝 {field}");
    }
    let mut fixture = Databases::new();
    fixture.target["preserved"]
        .as_object_mut()
        .unwrap()
        .remove("seaql_tenant_data_migrations");
    fixture.target["all_tables"] = json!(["sys_post", "ryframe_resource_ownership"]);
    assert!(fixture.images(&artifact).is_err());
}

fn object_item() -> Value {
    json!({
        "bucket":"uploads","source_key":"fixture-old/path/object","target_key":"fixture-new/path/object",
        "artifact":{"file":"objects/object.bin","bytes":4,"sha256":"a".repeat(64)},
        "metadata":{"ContentType":"application/octet-stream","CacheControl":null,"ContentDisposition":null,
            "ContentEncoding":null,"ContentLanguage":"en","Expires":null,"WebsiteRedirectLocation":null,"Metadata":{}},
    })
}

fn target(item: &Value) -> TargetObject<'_> {
    TargetObject {
        scope: TARGET,
        endpoint: ENDPOINT,
        item,
    }
}

fn capture(item: &Value) -> Value {
    json!({
        "format_version":1,"kind":"devex-clone-object-capture","side":"target","scope_id":TARGET,
        "endpoint":ENDPOINT,"region":"us-east-1","bucket":"uploads","key":item["target_key"],
        "expected":{"bytes":4,"sha256":"a".repeat(64)},"max_bytes":4,"transport":"rust-s3-sigv4",
        "clone_verified":false,"status":"captured","stored_metadata_verified":true,"bytes_verified":true,
        "get_header_consistent":true,"get_header_differences":[],"metadata":item["metadata"],
        "artifact":{"file":"object.bin","bytes":4,"sha256":"a".repeat(64)},
    })
}

fn descriptor(sha: &str) -> Value {
    let path = if cfg!(windows) {
        "D:/fixture/observation.json"
    } else {
        "/fixture/observation.json"
    };
    json!({"path":path,"bytes":128,"sha256":sha})
}

#[test]
fn object_absence_requires_the_bound_target_receipt_and_never_maps_failure_to_absence() {
    let generation = "f".repeat(64);
    let sha = "b".repeat(64);
    let item = object_item();
    let expected = object_pair(&generation, target(&item)).unwrap().0;
    let descriptor = descriptor(&sha);
    let mut receipt = json!({
        "format_version":1,"status":"object_absent",
        "resource":{"kind":"object","scope_id":TARGET,"endpoint":ENDPOINT,"bucket":"uploads","key":item["target_key"]},
        "capture":null,"remote_writes":0,
    });
    let verify = |receipt: &Value, digest: &str| {
        object_observed(
            &generation,
            target(&item),
            ObjectObservation::Absent {
                receipt,
                descriptor: &descriptor,
                raw_sha256: digest,
            },
        )
    };
    assert_eq!(verify(&receipt, &sha).unwrap(), expected);
    assert!(verify(&receipt, &"c".repeat(64)).is_err());
    receipt["status"] = json!("object_observation_failed");
    assert!(verify(&receipt, &sha).is_err());
    receipt["status"] = json!("object_absent");
    receipt["capture"] = json!({"sha256":sha});
    assert!(verify(&receipt, &sha).is_err());
    receipt["capture"] = Value::Null;
    receipt["resource"]["scope_id"] = json!(SOURCE);
    assert!(verify(&receipt, &sha).is_err());
}

#[test]
fn object_capture_binds_scope_and_content_hash_even_when_length_is_unchanged() {
    let generation = "f".repeat(64);
    let mut item = object_item();
    let mut captured = capture(&item);
    let expected = object_pair(&generation, target(&item)).unwrap().1;
    assert_eq!(
        object_observed(
            &generation,
            target(&item),
            ObjectObservation::Captured(&captured)
        )
        .unwrap(),
        expected
    );
    captured["artifact"]["sha256"] = json!("b".repeat(64));
    assert!(
        object_observed(
            &generation,
            target(&item),
            ObjectObservation::Captured(&captured)
        )
        .is_err()
    );
    let mut captured = capture(&item);
    captured["scope_id"] = json!(SOURCE);
    assert!(
        object_observed(
            &generation,
            target(&item),
            ObjectObservation::Captured(&captured)
        )
        .is_err()
    );
    item["target_key"] = json!("fixture-old/path/object");
    assert!(object_pair(&generation, target(&item)).is_err());
}

#[test]
fn object_metadata_hash_retains_exact_get_header_values_and_presence() {
    let generation = "f".repeat(64);
    let item = object_item();
    let mut captured = capture(&item);
    captured["get_header_consistent"] = json!(false);
    captured["get_header_differences"] = json!([{
        "field":"ContentLanguage","head_present":true,"head_value":"en","get_present":false,"get_value":null,
    }]);
    let with_absent_header = object_observed(
        &generation,
        target(&item),
        ObjectObservation::Captured(&captured),
    )
    .unwrap();
    assert_eq!(
        with_absent_header["metadata_sha256"],
        digest::plan_hash(&json!({
            "stored":item["metadata"],"get_header_differences":captured["get_header_differences"],
        }))
        .unwrap()
    );
    captured["get_header_differences"][0]["get_present"] = json!(true);
    let with_null_header = object_observed(
        &generation,
        target(&item),
        ObjectObservation::Captured(&captured),
    )
    .unwrap();
    assert_ne!(
        with_null_header["metadata_sha256"],
        with_absent_header["metadata_sha256"]
    );
    captured["get_header_consistent"] = json!(true);
    assert!(
        object_observed(
            &generation,
            target(&item),
            ObjectObservation::Captured(&captured)
        )
        .is_err()
    );
}
