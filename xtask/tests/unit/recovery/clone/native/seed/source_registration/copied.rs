use super::*;

fn inputs() -> [Value; 7] {
    let initialized = json!({"sha256":"a".repeat(64)});
    let exported = json!({"sha256":"b".repeat(64)});
    let session = json!({"initialized":initialized,"source_export":exported,
        "plan_sha256":"c".repeat(64),"generation_sha256":"d".repeat(64)});
    let plan = json!({"plan_sha256":session["plan_sha256"],
        "databases":[{"key":"control"}],"objects":[],"pending_target_actions":[{"kind":"requires-review"}]});
    let result = json!({"status":"data_steps_verified","source_export_sha256":exported["sha256"],
        "plan_sha256":session["plan_sha256"],"generation_sha256":session["generation_sha256"],
        "written_steps":1,"fresh_target_sha256":initialized["sha256"],"pending_target_actions":plan["pending_target_actions"]});
    let mut published = result.clone();
    published["continuation_receipt"] = json!({"bound":"original-controller-close"});
    let ledger = json!({"status":"ledger_evidence_complete","steps":{"database-example":{"phase":"confirmed"}}});
    [
        session,
        result,
        published,
        plan,
        ledger,
        initialized,
        exported,
    ]
}

fn check(values: &[Value; 7]) -> Result<()> {
    validate(
        &values[0], &values[1], &values[2], &values[3], &values[4], &values[5], &values[6],
    )
}

#[test]
fn completed_copy_requires_matching_outer_result_and_preserves_pending_api_only_actions() {
    let values = inputs();
    assert!(check(&values).is_ok());
    for (index, field, value) in [
        (2, "written_steps", json!(2)),
        (2, "extra", json!(true)),
        (1, "written_steps", json!(2)),
        (1, "fresh_target_sha256", json!("0".repeat(64))),
        (0, "initialized", json!({"sha256":"0".repeat(64)})),
    ] {
        let mut changed = values.clone();
        changed[index][field] = value;
        assert!(check(&changed).is_err());
    }
    let mut changed = values;
    changed[1]["pending_target_actions"] = json!([]);
    changed[2]["pending_target_actions"] = json!([]);
    assert!(check(&changed).is_err());
}

#[test]
fn zero_write_copy_requires_complete_unchanged_database_set_and_no_objects_or_unknown_steps() {
    let mut values = inputs();
    let key = format!(
        "database-{}",
        &digest::plan_hash(&json!({"key":"control"})).unwrap()[..32]
    );
    values[1]["written_steps"] = json!(0);
    values[1]["unchanged_databases"] = json!(1);
    values[1]["unchanged_database_images"] = json!({key:{"bound":"full-image"}});
    values[2] = values[1].clone();
    values[4] = json!({"status":"partial","steps":{}});
    assert!(check(&values).is_ok());
    for (index, field, value) in [
        (4, "status", json!("needs_reconciliation")),
        (3, "objects", json!([{"key":"unwritten-object"}])),
        (4, "steps", json!({"unknown":{"phase":"unknown"}})),
    ] {
        let mut changed = values.clone();
        changed[index][field] = value;
        assert!(check(&changed).is_err());
    }
    values[1]["unchanged_database_images"] = json!({"different-database":{}});
    values[2] = values[1].clone();
    assert!(check(&values).is_err());
}
