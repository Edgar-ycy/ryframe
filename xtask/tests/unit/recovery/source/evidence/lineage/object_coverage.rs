use super::*;

fn inputs(probe: bool) -> (Value, Value) {
    let mut objects = Vec::new();
    let mut files = Vec::new();
    for index in 0..256 {
        let relative = format!("system/object-{index:03}.bin");
        let sha256 = format!("{index:064x}");
        files.push(json!({"file_path":relative,"bytes":4194304,"sha256":sha256}));
        objects.push(
            json!({"bucket":"uploads","source_key":format!("fixture-source/{relative}"),
            "target_key":format!("fixture-target/{relative}"),
            "artifact":{"bytes":4194304,"sha256":sha256}}),
        );
    }
    if probe {
        objects.push(
            json!({"bucket":"exports","source_key":"fixture-source/system/probe.txt",
            "target_key":"fixture-target/system/probe.txt",
            "artifact":{"bytes":4,"sha256":"a".repeat(64)}}),
        );
    }
    (
        json!({"tenants":[{"files":files}]}),
        json!({"target_scope":"fixture-target","objects":objects}),
    )
}

fn receipt(additional: u64) -> Value {
    json!({"business_objects":256,"additional_objects":additional,
        "additional_objects_downloaded":true,"all_scoped_keys_verified":true})
}

#[test]
fn coverage_matches_both_zero_complement_and_the_existing_probe() {
    for (probe, additional) in [(false, 0), (true, 1)] {
        let (dataset, plan) = inputs(probe);
        assert!(verify_object_coverage(&receipt(additional), &dataset, 256, &plan).is_ok());
        assert!(verify_object_coverage(&receipt(1 - additional), &dataset, 256, &plan).is_err());
    }
}

#[test]
fn receipt_requires_real_five_bucket_coverage_and_all_complement_downloads() {
    let (dataset, plan) = inputs(true);
    for field in ["all_scoped_keys_verified", "additional_objects_downloaded"] {
        let mut value = receipt(1);
        value[field] = json!(false);
        assert!(verify_object_coverage(&value, &dataset, 256, &plan).is_err());
    }
    assert!(verify_object_coverage(&receipt(1), &dataset, 255, &plan).is_err());
}

#[test]
fn wrong_bucket_same_length_digest_change_and_duplicate_cannot_cover_samples() {
    let (dataset, plan) = inputs(false);
    let mut wrong_bucket = plan.clone();
    wrong_bucket["objects"][0]["bucket"] = json!("avatar");
    assert!(verify_object_coverage(&receipt(0), &dataset, 256, &wrong_bucket).is_err());
    let mut changed = plan.clone();
    changed["objects"][0]["artifact"]["sha256"] = json!("b".repeat(64));
    assert!(verify_object_coverage(&receipt(0), &dataset, 256, &changed).is_err());
    let mut duplicate = dataset;
    duplicate["tenants"][0]["files"][1] = duplicate["tenants"][0]["files"][0].clone();
    assert!(verify_object_coverage(&receipt(0), &duplicate, 256, &plan).is_err());
}
