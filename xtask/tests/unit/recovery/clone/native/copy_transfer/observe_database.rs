use super::*;
use crate::recovery::clone::native::target_verify::input::request::KEYS;

const SOURCE: &str = "source-fixture";
const TARGET: &str = "target-fixture";
const SERVER: &str = "67cd8d4a-fe90-11ef-98fd-5847ca786499";

fn declarations(prefix: &str) -> BTreeMap<String, Value> {
    KEYS.into_iter()
        .map(|(key, kind, mode)| {
            (
                key.to_owned(),
                json!({
                    "key":key,"kind":kind,"mode":mode,"server_uuid":SERVER,
                    "database":format!("{prefix}_{}",key.replace('-', "_")),
                }),
            )
        })
        .collect()
}

fn tables(byte: &str, rows: u64) -> Value {
    json!({"sys_post":{"rows":rows,"sha256":byte.repeat(64)}})
}

fn physical(declared: &BTreeMap<String, Value>, scope: &str, rows: u64) -> Value {
    let fingerprints = json!({
        "control_schema_fingerprint":"a".repeat(16), "tenant_schema_fingerprint":"b".repeat(64)
    });
    Value::Object(declared.values().map(|db| {
        let combined = db["kind"] == "combined";
        let mut schema = fingerprints.clone();
        if !combined {
            schema["control_schema_fingerprint"] = Value::Null;
        }
        let name = db["database"].as_str().unwrap();
        (name.to_owned(), json!({
            "resource":{"kind":"database","scope_id":scope,"server_uuid":SERVER,"database":name},
            "schema_sha256":digest::plan_hash(&schema).unwrap(),
            "tables":tables(if rows == 0 {"c"} else {"d"}, rows),
            "preserved":{
                "ryframe_resource_ownership":{"rows":if combined {2} else {1},"sha256":"e".repeat(64)},
                "seaql_tenant_data_migrations":{"rows":6,"sha256":"f".repeat(64)},
            },
            "all_tables":["ryframe_resource_ownership","seaql_tenant_data_migrations","sys_post"],
            "ownership":model::expected_owners(scope, combined),
        }))
    }).collect())
}

fn export_inventory(declared: &BTreeMap<String, Value>) -> Value {
    let entries = declared.iter().map(|(key, db)| json!({
        "key":key,"kind":db["kind"],"server_uuid":db["server_uuid"],"database":db["database"],
        "shared":db["mode"] == "shared","placements":[],
        "tables":[{"table":"sys_post","rows":17,"sha256":"d".repeat(64)}],
    })).collect::<Vec<_>>();
    json!({
        "scope_id":SOURCE,"control_schema_fingerprint":"a".repeat(16),
        "tenant_schema_fingerprint":"b".repeat(64),"databases":entries,
    })
}

fn receipt() -> Value {
    let root = crate::workspace::root_dir().join(".local-tests");
    json!({"path":root.join("inventory.json"),"bytes":17,"sha256":"0".repeat(64)})
}

fn evidence() -> Vec<Value> {
    ["before", "after"].into_iter().flat_map(|phase| KEYS.map(|(key, _, _)| {
        json!({"path":format!("{phase}-target-{key}.json"),"type":"file","bytes":42,"sha256":"e".repeat(64)})
    })).collect()
}

fn captured(
    declared: &BTreeMap<String, Value>,
    scope: &str,
    side: CloneSide,
    rows: u64,
) -> Captured {
    let images = physical(declared, scope, rows);
    Captured {
        binding: json!({"side":side.as_str()}),
        inventory: json!({
            "side":side.as_str(),"scope_id":scope,"observations":index_physical(&images,declared,scope).unwrap(),
        }),
        receipt: receipt(),
        observations: serde_json::from_value(images).unwrap(),
        evidence_files: evidence(),
    }
}

#[test]
fn normalized_inventory_keeps_both_rounds_and_requires_the_physical_bijection() {
    let declared = declarations("source");
    let source = captured(&declared, SOURCE, CloneSide::Source, 17);
    let result = Observed::from_capture(source, &declared, SOURCE, CloneSide::Source).unwrap();
    assert_eq!(result.observations.len(), 4);
    assert_eq!(
        result.observations["shared-control"]["resource"]["database"],
        "source_shared_control"
    );
    assert_eq!(result.receipt, receipt());
    assert_eq!(result.evidence_files, evidence());

    let mut wrong_index = captured(&declared, SOURCE, CloneSide::Source, 17);
    let image = wrong_index
        .observations
        .remove("source_shared_control")
        .unwrap();
    wrong_index
        .observations
        .insert("shared-control".into(), image);
    assert!(Observed::from_capture(wrong_index, &declared, SOURCE, CloneSide::Source).is_err());
    let mut wrong_receipt = captured(&declared, SOURCE, CloneSide::Source, 17);
    wrong_receipt.inventory["observations"]["shared"]["tables"] = tables("1", 17);
    assert!(Observed::from_capture(wrong_receipt, &declared, SOURCE, CloneSide::Source).is_err());
}

#[test]
fn source_must_match_old_export_schema_and_every_saved_row_without_losing_preserved_rows() {
    let declared = declarations("source");
    let source = captured(&declared, SOURCE, CloneSide::Source, 17);
    let source = Observed::from_capture(source, &declared, SOURCE, CloneSide::Source).unwrap();
    let inventory = export_inventory(&declared);
    source_matches_export(&source.observations, &declared, SOURCE, &inventory).unwrap();
    let projection = source_projection(&source.observations);
    assert_eq!(
        projection["shared"]["preserved"]["seaql_tenant_data_migrations"]["rows"],
        6
    );

    let mut drift = inventory.clone();
    drift["tenant_schema_fingerprint"] = json!("1".repeat(64));
    assert!(source_matches_export(&source.observations, &declared, SOURCE, &drift).is_err());
    drift = inventory.clone();
    drift["databases"][0]["tables"][0]["rows"] = json!(16);
    assert!(source_matches_export(&source.observations, &declared, SOURCE, &drift).is_err());
    drift = inventory.clone();
    drift["databases"][0]["database"] = json!("other_physical");
    assert!(source_matches_export(&source.observations, &declared, SOURCE, &drift).is_err());

    // 旧导出只要求其原表记录得到核实，额外保留表由完整会话像绑定。
    let mut source_drift = source.observations;
    source_drift.get_mut("shared").unwrap()["preserved"]["seaql_tenant_data_migrations"]["rows"] =
        json!(7);
    source_matches_export(&source_drift, &declared, SOURCE, &inventory).unwrap();
    assert_ne!(source_projection(&source_drift), projection);
}

#[test]
fn final_target_changes_only_business_tables_and_keeps_initial_identity_and_owner() {
    let source = declarations("source");
    let target = declarations("target");
    let source = Observed::from_capture(
        captured(&source, SOURCE, CloneSide::Source, 17),
        &source,
        SOURCE,
        CloneSide::Source,
    )
    .unwrap();
    let initial = json!({
        "status":"fresh_target_initialized","scope_id":TARGET,"controlled_generation_never_started":true,
        "inventory":{"receipt":receipt(),"observations":physical(&target, TARGET, 0)},
    });
    let (initialized, initialized_receipt) = initialized_images(&initial, &target, TARGET).unwrap();
    let baseline = Baseline {
        source,
        initialized,
        initialized_receipt,
    };
    let artifact = "1".repeat(64);
    let basis = baseline.basis("shared", &artifact).unwrap();
    assert_eq!(basis.source["tables"]["sys_post"]["rows"], 17);
    assert_eq!(basis.initialized["tables"]["sys_post"]["rows"], 0);
    assert_eq!(basis.artifact_sha256, artifact);
    assert_eq!(*baseline.initialized_receipt(), receipt());

    let mut live = Observed::from_capture(
        captured(&target, TARGET, CloneSide::Target, 17),
        &target,
        TARGET,
        CloneSide::Target,
    )
    .unwrap();
    baseline.verify_complete_target(&live).unwrap();
    live.observations.get_mut("shared").unwrap()["preserved"]["seaql_tenant_data_migrations"]["rows"] =
        json!(7);
    assert!(baseline.verify_complete_target(&live).is_err());

    let mut initial_drift = initial.clone();
    initial_drift["inventory"]["observations"]["target_shared_control"]["ownership"] = json!([]);
    assert!(initialized_images(&initial_drift, &target, TARGET).is_err());
    initial_drift = initial;
    initial_drift["inventory"]["observations"]
        .as_object_mut()
        .unwrap()
        .remove("target_shared");
    assert!(initialized_images(&initial_drift, &target, TARGET).is_err());
}
