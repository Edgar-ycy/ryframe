use super::*;

#[test]
fn binds_generation_with_the_same_canonical_bytes_as_existing_copy_records() {
    let source = json!({"bytes":3,"path":"source.json","sha256":"a".repeat(64)});
    let target = json!({"bytes":5,"path":"target.json","sha256":"b".repeat(64)});
    let mut files = Files {
        backend: Path::new(""),
        root: PathBuf::new(),
        source_export: &source,
        initialized: &target,
        source_environment: BTreeMap::from([("APP_ENV".into(), "test".into())]),
        target_environment: BTreeMap::from([
            ("APP_ENV".into(), "test".into()),
            ("TMP".into(), "D:/临时".into()),
            ("TEMP".into(), "D:/临时".into()),
        ]),
        protected: Inputs::open([]).unwrap(),
    };
    let facts = Facts {
        source_generation: &json!({"source":"源"}),
        target_generation: &json!({"target":"目标"}),
        source_observation: &json!({"shared-control":{},"shared":{},"dedicated-a":{},"dedicated-b":{}}),
        declaration: &Value::Null,
    };
    let plan = json!({"plan_sha256":"c".repeat(64)});
    let record = files.value(&facts, &plan).unwrap();
    assert_eq!(
        record["generation_sha256"],
        "ea76cd3e39ae92c6fc03c01dc14aaae21c6d44c78073a7a4a9412f52a6962a98"
    );
    assert_eq!(record["source_observation"], *facts.source_observation);
    assert_eq!(record["clone_verified"], false);
    assert_eq!(record["restore_qualified"], false);
    files
        .target_environment
        .insert("TEMP".into(), "D:/漂移".into());
    assert_ne!(
        files.value(&facts, &plan).unwrap()["generation_sha256"],
        record["generation_sha256"]
    );
}
